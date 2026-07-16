#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, State};

struct RunState(AtomicBool);

const BOUNDS_BACKUP_DIR: &str = "batch_rftime_backups";
const ORIGINAL_BOUNDS_BACKUP: &str = "batch_rftime_original.rftime";

#[derive(Default, Deserialize, Serialize)]
struct Settings {
    #[serde(default)]
    last_project: String,
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    last_backups: HashMap<String, String>,
    #[serde(default)]
    backup_labels: HashMap<String, HashMap<String, String>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectOutputs {
    acq_time: bool,
    long_csv: bool,
    misc_data: bool,
    pdf_plots: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectSummary {
    name: String,
    path: String,
    mzml_count: usize,
    transition_count: usize,
    transition_file: Option<String>,
    worker_name: Option<String>,
    issues: Vec<String>,
    outputs: ProjectOutputs,
    can_run: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VisualizerPoint {
    rt: f32,
    intensity: f32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VisualizerWindow {
    label: String,
    start_index: usize,
    end_index: usize,
    rt_start: f32,
    rt_end: f32,
    apex_rt: f32,
    height: f32,
    area: f32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VisualizerSample {
    sample_id: String,
    display_name: String,
    points: Vec<VisualizerPoint>,
    wells: Vec<VisualizerWindow>,
}

struct BatchLayout {
    plate: Option<usize>,
    sip: Option<usize>,
    seq: usize,
    row: usize,
    col: usize,
    time: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VisualizerTransitionData {
    transition: String,
    samples: Vec<VisualizerSample>,
    sample_count: usize,
    well_count: usize,
    global_rt_min: f32,
    global_rt_max: f32,
    global_intensity_max: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoundEdit {
    sample_id: String,
    well_label: String,
    rt_start: f32,
    rt_end: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupList {
    backups: Vec<String>,
    last: Option<String>,
    labels: HashMap<String, String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveBoundsResult {
    written: usize,
    backup: Option<String>,
    project: ProjectSummary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupState {
    project: Option<ProjectSummary>,
    remembered_path: Option<String>,
    needs_reselection: bool,
    theme: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerOutput {
    stream: String,
    line: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunStatus {
    status: String,
}

struct BinReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BinReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| "the visualizer data file is too large".to_string())?;
        if end > self.data.len() {
            return Err("the visualizer data file ended unexpectedly".to_string());
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn skip(&mut self, len: usize) -> Result<(), String> {
        self.take(len).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        let bytes: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| "the visualizer data file is malformed".to_string())?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn f32(&mut self) -> Result<f32, String> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| "the visualizer data file is malformed".to_string())?;
        Ok(f32::from_le_bytes(bytes))
    }

    fn string(&mut self) -> Result<String, String> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != 0 {
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            return Err("the visualizer data string terminator was missing".to_string());
        }
        let bytes = &self.data[start..self.pos];
        self.pos += 1;
        String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string())
    }
}

// stores gui settings in the app config folder
fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("settings.json"))
        .map_err(|error| error.to_string())
}

// reads saved gui settings if present
fn read_settings(app: &AppHandle) -> Option<Settings> {
    let contents = fs::read(settings_path(app).ok()?).ok()?;
    serde_json::from_slice(&contents).ok()
}

// writes gui settings atomically enough for small files
fn write_settings(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let settings_path = settings_path(app)?;
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(settings_path, json).map_err(|error| error.to_string())
}

// remembers the last selected project
fn save_project(app: &AppHandle, project: &Path) -> Result<(), String> {
    let mut settings = read_settings(app).unwrap_or_default();
    settings.last_project = project.to_string_lossy().into_owned();
    write_settings(app, &settings)
}

// appends .exe only on windows
fn worker_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

fn is_file(path: &Path) -> bool {
    path.is_file()
}

// finds the repository root in dev builds
fn source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

// lists worker locations from most to least portable
fn candidate_worker_paths(project: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("RFKIT_WORKER").map(PathBuf::from) {
        candidates.push(path);
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        candidates.push(directory.join(worker_name("RFkit-worker")));
        candidates.push(directory.join(worker_name("RFkit")));
    }

    let root = source_root();
    candidates.push(
        root.join("target")
            .join("release")
            .join(worker_name("RFkit")),
    );
    candidates.push(root.join("target").join("debug").join(worker_name("RFkit")));

    candidates.push(project.join(worker_name("RFkit")));
    candidates.push(project.join(worker_name("RFkit-optimized")));
    candidates.push(project.join(if cfg!(windows) {
        "RFkit_windows.exe"
    } else {
        "RFkit_macOS"
    }));
    candidates
}

fn find_worker(project: &Path) -> Option<PathBuf> {
    candidate_worker_paths(project)
        .into_iter()
        .find(|path| is_file(path))
}

// resolves param.txt paths against the dataset folder
fn project_file(project: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        project.join(path)
    }
}

// shows paths relative to the dataset when possible
fn display_name(project: &Path, path: &Path) -> String {
    path.strip_prefix(project)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

// rejects backup names that could leave the backup folder
fn safe_backup_name(name: &str) -> Result<&str, String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || !name.ends_with(".rftime")
    {
        return Err("the backup name is invalid".to_string());
    }
    Ok(name)
}

// keeps renamed backup labels small and printable
fn safe_backup_label(label: &str) -> Result<String, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("the backup label cannot be blank".to_string());
    }
    if label.chars().count() > 80 || label.chars().any(char::is_control) {
        return Err("the backup label is too long or contains invalid characters".to_string());
    }
    Ok(label.to_string())
}

// keeps original at the top of the backup menu
fn order_backups(mut backups: Vec<String>) -> Vec<String> {
    backups.sort_unstable();
    if let Some(index) = backups
        .iter()
        .position(|name| name == ORIGINAL_BOUNDS_BACKUP)
    {
        let original = backups.remove(index);
        backups.insert(0, original);
    }
    backups
}

// saves the first untouched batch.rftime
fn capture_original_bounds(project: &Path) -> Result<(), String> {
    let source = project.join("batch.rftime");
    if !source.is_file() {
        return Ok(());
    }
    let directory = project.join(BOUNDS_BACKUP_DIR);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    fs::copy(source, directory.join(ORIGINAL_BOUNDS_BACKUP))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

// creates the protected original backup if missing
fn ensure_original_bounds(project: &Path) -> Result<(), String> {
    if !project
        .join(BOUNDS_BACKUP_DIR)
        .join(ORIGINAL_BOUNDS_BACKUP)
        .is_file()
    {
        capture_original_bounds(project)?;
    }
    Ok(())
}

// copies current batch.rftime into a named backup
fn backup_bounds(project: &Path, name: &str) -> Result<Option<String>, String> {
    let name = safe_backup_name(name)?;
    let source = project.join("batch.rftime");
    if !source.is_file() {
        return Ok(None);
    }
    let directory = project.join(BOUNDS_BACKUP_DIR);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    fs::copy(&source, directory.join(name)).map_err(|error| error.to_string())?;
    Ok(Some(name.to_string()))
}

// reads the retention-time offset from param.txt
fn read_offset(project: &Path) -> Result<f32, String> {
    let param = fs::read_to_string(project.join("param.txt")).map_err(|error| error.to_string())?;
    let value = param
        .parse::<toml::Table>()
        .map_err(|error| error.to_string())?;
    value
        .get("offset")
        .and_then(|value| {
            value
                .as_float()
                .or_else(|| value.as_integer().map(|value| value as f64))
        })
        .map(|value| value as f32)
        .ok_or_else(|| "offset is missing from param.txt".to_string())
}

// extracts the numeric sequence from sequencex.mzML
fn sample_sequence(sample_id: &str) -> Result<u8, String> {
    sample_id
        .trim()
        .strip_prefix("sequence")
        .and_then(|value| {
            value
                .strip_suffix(".mzML")
                .or_else(|| value.strip_suffix(".mzml"))
        })
        .ok_or_else(|| format!("sample {sample_id} is not an RFkit sequence sample"))?
        .parse::<u8>()
        .map_err(|error| error.to_string())
}

// keeps spaced plate names intact in tabbed data rows
fn split_batch_line(line: &str) -> Vec<String> {
    if line.contains('\t') {
        line.split('\t')
            .map(|cell| cell.trim().to_string())
            .collect()
    } else {
        line.split_whitespace().map(str::to_string).collect()
    }
}

// reads batch.rftime columns by name when headers are available
fn batch_layout(rows: &[Vec<String>]) -> BatchLayout {
    let headers: Vec<String> = rows
        .first()
        .map(|row| {
            row.iter()
                .map(|cell| cell.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let index = |name: &str| headers.iter().position(|header| header == name);
    BatchLayout {
        plate: index("plate"),
        sip: index("sip"),
        seq: index("seq").unwrap_or(2),
        row: index("row").unwrap_or(3),
        col: index("col").unwrap_or(4),
        time: index("siptime").or_else(|| index("time")).unwrap_or(5),
    }
}

// maps each sequence file back to its plate label
fn plate_by_sequence(project: &Path) -> HashMap<u8, String> {
    let path = project.join("batch.rftime");
    let Ok(contents) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let rows: Vec<Vec<String>> = contents.lines().map(split_batch_line).collect();
    let layout = batch_layout(&rows);
    let Some(plate_index) = layout.plate else {
        return HashMap::new();
    };
    let mut plates = HashMap::new();
    for row in rows.iter().skip(2) {
        if row.len() <= layout.seq || row.len() <= plate_index {
            continue;
        }
        let Ok(sequence) = row[layout.seq].trim().parse::<u8>() else {
            continue;
        };
        let plate = row[plate_index].trim();
        if !plate.is_empty() {
            plates.entry(sequence).or_insert_with(|| plate.to_string());
        }
    }
    plates
}

// maps each well to the acquisition sip order from batch.rftime
fn sip_order_by_well(project: &Path) -> HashMap<(u8, u8, u8), u32> {
    let path = project.join("batch.rftime");
    let Ok(contents) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let rows: Vec<Vec<String>> = contents.lines().map(split_batch_line).collect();
    let layout = batch_layout(&rows);
    let Some(sip_index) = layout.sip else {
        return HashMap::new();
    };

    let mut order = HashMap::new();
    for row in rows.iter().skip(2) {
        if row.len() <= layout.seq
            || row.len() <= layout.row
            || row.len() <= layout.col
            || row.len() <= sip_index
        {
            continue;
        }
        if row[layout.row].trim() == "3001" {
            continue;
        }
        let (Ok(sequence), Ok(well_row), Ok(well_column), Ok(sip)) = (
            row[layout.seq].trim().parse::<u8>(),
            row[layout.row].trim().parse::<u8>(),
            row[layout.col].trim().parse::<u8>(),
            row[sip_index].trim().parse::<u32>(),
        ) else {
            continue;
        };
        order.insert((sequence, well_row, well_column), sip);
    }
    order
}

// makes sequence labels match the plate wording in batch.rftime
fn sample_display_name(sample_id: &str, plates: &HashMap<u8, String>) -> String {
    sample_sequence(sample_id)
        .ok()
        .and_then(|sequence| {
            plates.get(&sequence).map(|plate| {
                let plate_number = plate
                    .chars()
                    .filter(char::is_ascii_digit)
                    .collect::<String>();
                if plate_number.is_empty() {
                    plate.to_string()
                } else {
                    format!("plate {plate_number}")
                }
            })
        })
        .unwrap_or_else(|| sample_id.to_string())
}

// reads labels like (4, 12) from the RFkit plot binary
fn parse_well_label(label: &str) -> Result<(u8, u8), String> {
    let label = label.trim().trim_start_matches('(').trim_end_matches(')');
    let (row, column) = label
        .split_once(',')
        .ok_or_else(|| format!("well label {label} is malformed"))?;
    Ok((
        row.trim()
            .parse::<u8>()
            .map_err(|error| error.to_string())?,
        column
            .trim()
            .parse::<u8>()
            .map_err(|error| error.to_string())?,
    ))
}

// writes updated integration times back into the parsed row
fn update_time_cell(
    cells: &mut Vec<String>,
    time_index: usize,
    seconds: f32,
) -> Result<(), String> {
    if cells.len() <= time_index {
        return Err("batch.rftime does not contain a time column".to_string());
    }
    cells[time_index] = format!("{seconds:.3}");
    Ok(())
}

// applies dragged bounds to the matching start and sensor rows
fn save_bounds_to_batch(project: &Path, edits: &[BoundEdit]) -> Result<usize, String> {
    if edits.is_empty() {
        return Ok(0);
    }
    let path = project.join("batch.rftime");
    if !path.is_file() {
        return Err("batch.rftime was not found".to_string());
    }
    let offset = read_offset(project)?;
    let contents = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let mut rows: Vec<Vec<String>> = contents.lines().map(split_batch_line).collect();
    if rows.len() < 3 {
        return Err("batch.rftime is missing acquisition rows".to_string());
    }

    let layout = batch_layout(&rows);
    let mut starts = HashMap::<(u8, u8, u8), usize>::new();
    let mut ends = HashMap::<(u8, u8, u8), usize>::new();
    let mut current: Option<(u8, u8, u8)> = None;
    for (index, row) in rows.iter().enumerate().skip(2) {
        if row.len() <= layout.seq || row.len() <= layout.row || row.len() <= layout.col {
            continue;
        }
        let Ok(sequence) = row[layout.seq].trim().parse::<u8>() else {
            continue;
        };
        if row[layout.row].trim() == "3001" {
            if let Some(key) = current.take() {
                ends.insert(key, index);
            }
        } else if let (Ok(well_row), Ok(well_column)) = (
            row[layout.row].trim().parse::<u8>(),
            row[layout.col].trim().parse::<u8>(),
        ) {
            let key = (sequence, well_row, well_column);
            starts.insert(key, index);
            current = Some(key);
        }
    }

    let mut written = 0;
    for edit in edits {
        let sequence = sample_sequence(&edit.sample_id)?;
        let (well_row, well_column) = parse_well_label(&edit.well_label)?;
        let key = (sequence, well_row, well_column);
        let start_index = *starts.get(&key).ok_or_else(|| {
            format!(
                "well {} in {} was not found in batch.rftime",
                edit.well_label, edit.sample_id
            )
        })?;
        let end_index = *ends.get(&key).ok_or_else(|| {
            format!(
                "well {} in {} is missing an end row in batch.rftime",
                edit.well_label, edit.sample_id
            )
        })?;
        let (low, high) = if edit.rt_start <= edit.rt_end {
            (edit.rt_start, edit.rt_end)
        } else {
            (edit.rt_end, edit.rt_start)
        };
        let start_seconds = (low - offset) * 60.0;
        let end_seconds = (high - offset) * 60.0;
        update_time_cell(&mut rows[start_index], layout.time, start_seconds)?;
        update_time_cell(&mut rows[end_index], layout.time, end_seconds)?;
        written += 1;
    }

    let mut output = rows
        .iter()
        .map(|row| row.join("\t"))
        .collect::<Vec<_>>()
        .join("\n");
    output.push('\n');
    fs::write(path, output).map_err(|error| error.to_string())?;
    Ok(written)
}

// counts mzml inputs recursively
fn count_mzml_files(directory: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![directory.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = fs::read_dir(path) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mzML"))
            {
                count += 1;
            }
        }
    }
    count
}

// counts usable transition rows after the csv header
fn count_transition_rows(path: &Path) -> usize {
    let Ok(contents) = fs::read_to_string(path) else {
        return 0;
    };
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .skip(1)
        .count()
}

// checks whether expected output files exist
fn has_matching_file(project: &Path, prefix: &str, extension: &str) -> bool {
    fs::read_dir(project)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .any(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(prefix)
                            && name
                                .to_ascii_lowercase()
                                .ends_with(&extension.to_ascii_lowercase())
                    })
        })
}

// lists plot binaries in numeric sequence order
fn plot_files(project: &Path) -> Result<Vec<PathBuf>, String> {
    let misc = project.join("misc");
    if !project.is_dir() {
        return Err("the selected dataset folder does not exist".to_string());
    }
    if !misc.is_dir() {
        return Err("run RFkit before opening the visualizer".to_string());
    }
    let mut files: Vec<PathBuf> = fs::read_dir(misc)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("plot_") && name.ends_with(".bin"))
        })
        .collect();
    files.sort_by(|left, right| {
        let left_id = sample_id_from_plot(left);
        let right_id = sample_id_from_plot(right);
        match (sample_sequence(&left_id), sample_sequence(&right_id)) {
            (Ok(left_sequence), Ok(right_sequence)) => left_sequence.cmp(&right_sequence),
            _ => left.cmp(right),
        }
    });
    if files.is_empty() {
        return Err("no RFkit plot data was found in misc".to_string());
    }
    Ok(files)
}

// strips plot_ and .bin from an RFkit plot filename
fn sample_id_from_plot(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("plot_"))
        .and_then(|name| name.strip_suffix(".bin"))
        .unwrap_or("sample")
        .to_string()
}

// skips wells for transitions that are not being rendered
fn skip_wells(reader: &mut BinReader<'_>) -> Result<(), String> {
    let well_count = reader.u8()?;
    for _ in 0..well_count {
        let _ = reader.string()?;
        reader.skip(4)?;
    }
    Ok(())
}

// skips a full transition block in the binary plot file
fn skip_transition(reader: &mut BinReader<'_>) -> Result<(), String> {
    let point_count = usize::from(reader.u16()?);
    reader.skip(point_count * 8)?;
    skip_wells(reader)
}

// reads the transition names stored in a plot binary
fn transition_names(path: &Path) -> Result<Vec<String>, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let mut reader = BinReader::new(&bytes);
    let transition_count = reader.u8()?;
    let mut names = Vec::with_capacity(usize::from(transition_count));
    for _ in 0..transition_count {
        let name = reader.string()?;
        names.push(name);
        skip_transition(&mut reader)?;
    }
    Ok(names)
}

// converts raw RFkit indices into a renderable integration window
fn parse_window(
    points: &[VisualizerPoint],
    label: String,
    raw_start: u16,
    raw_end: u16,
) -> VisualizerWindow {
    if points.is_empty() {
        return VisualizerWindow {
            label,
            start_index: 0,
            end_index: 0,
            rt_start: 0.0,
            rt_end: 0.0,
            apex_rt: 0.0,
            height: 0.0,
            area: 0.0,
        };
    }
    let last = points.len().saturating_sub(1);
    let mut start = usize::from(raw_start.saturating_sub(1)).min(last);
    let mut end = usize::from(raw_end.saturating_sub(1)).min(last);
    if end < start {
        std::mem::swap(&mut start, &mut end);
    }
    let rt_start = points.get(start).map_or(0.0, |point| point.rt);
    let rt_end = points.get(end).map_or(rt_start, |point| point.rt);
    let mut apex_rt = rt_start;
    let mut height = 0.0_f32;
    let mut area = 0.0_f32;
    for point in &points[start..=end] {
        if point.intensity >= height {
            height = point.intensity;
            apex_rt = point.rt;
        }
    }
    for pair in points[start..=end].windows(2) {
        area += (pair[0].intensity + pair[1].intensity) * (pair[1].rt - pair[0].rt) * 30.0;
    }
    VisualizerWindow {
        label,
        start_index: start,
        end_index: end,
        rt_start,
        rt_end,
        apex_rt,
        height,
        area,
    }
}

fn parse_selected_transition(
    path: &Path,
    transition: &str,
    plates: &HashMap<u8, String>,
    sip_order: &HashMap<(u8, u8, u8), u32>,
) -> Result<Option<VisualizerSample>, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let mut reader = BinReader::new(&bytes);
    let transition_count = reader.u8()?;
    for _ in 0..transition_count {
        let name = reader.string()?;
        if name != transition {
            skip_transition(&mut reader)?;
            continue;
        }

        let point_count = usize::from(reader.u16()?);
        let mut points = Vec::with_capacity(point_count);
        for _ in 0..point_count {
            points.push(VisualizerPoint {
                rt: reader.f32()?,
                intensity: reader.f32()?,
            });
        }
        let well_count = reader.u8()?;
        let mut wells = Vec::with_capacity(usize::from(well_count));
        for well_index in 0..usize::from(well_count) {
            let label = reader.string()?;
            let start = reader.u16()?;
            let end = reader.u16()?;
            let well = parse_window(&points, label, start, end);
            if well.end_index > well.start_index
                && well.rt_end > well.rt_start
                && well.height.is_finite()
                && well.height > 0.0
                && well.area.is_finite()
                && well.area > 0.0
            {
                wells.push((well_index, well));
            }
        }
        let sample_id = sample_id_from_plot(path);
        if let Ok(sequence) = sample_sequence(&sample_id) {
            wells.sort_by_key(|(well_index, well)| {
                let sip = parse_well_label(&well.label)
                    .ok()
                    .and_then(|(well_row, well_column)| {
                        sip_order.get(&(sequence, well_row, well_column)).copied()
                    })
                    .unwrap_or(u32::MAX);
                (sip, *well_index)
            });
        }
        return Ok(Some(VisualizerSample {
            display_name: sample_display_name(&sample_id, plates),
            sample_id,
            points,
            wells: wells.into_iter().map(|(_, well)| well).collect(),
        }));
    }
    Ok(None)
}

// validates a dataset and summarizes its outputs
fn inspect_project(project: &Path) -> ProjectSummary {
    let mut issues = Vec::new();
    let mut transition_file = None;
    let mut transition_count = 0;
    let param_path = project.join("param.txt");
    let mzml_dir = project.join("mzml_dir");
    let mzml_count = count_mzml_files(&mzml_dir);
    let worker = find_worker(project);

    if !param_path.is_file() {
        issues.push("param.txt was not found".to_string());
    } else {
        match fs::read_to_string(&param_path)
            .map_err(|error| error.to_string())
            .and_then(|contents| {
                contents
                    .parse::<toml::Table>()
                    .map_err(|error| error.to_string())
            }) {
            Ok(parameters) => {
                if let Some(value) = parameters
                    .get("transition_list")
                    .and_then(toml::Value::as_str)
                {
                    let path = project_file(project, value);
                    transition_file = Some(display_name(project, &path));
                    if path.is_file() {
                        transition_count = count_transition_rows(&path);
                    } else {
                        issues.push("the transition list was not found".to_string());
                    }
                } else {
                    issues.push("transition_list is missing from param.txt".to_string());
                }

                if parameters
                    .get("offset")
                    .and_then(toml::Value::as_float)
                    .is_none()
                {
                    issues.push("offset is missing from param.txt".to_string());
                }
            }
            Err(error) => issues.push(format!("param.txt could not be read: {error}")),
        }
    }

    if !project.join("batch.rftime").is_file() {
        issues.push("batch.rftime was not found".to_string());
    }
    if !mzml_dir.is_dir() {
        issues.push("mzml_dir was not found".to_string());
    } else if mzml_count == 0 {
        issues.push("no mzML files were found in mzml_dir".to_string());
    }
    if worker.is_none() {
        issues.push("the RFkit processing engine was not found".to_string());
    }

    let outputs = ProjectOutputs {
        acq_time: project.join("acq_time.csv").is_file(),
        long_csv: project.join("long.csv").is_file(),
        misc_data: project.join("misc").is_dir(),
        pdf_plots: has_matching_file(project, "plot_", ".pdf"),
    };

    ProjectSummary {
        name: project.file_name().map_or_else(
            || "project".to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        path: project.to_string_lossy().into_owned(),
        mzml_count,
        transition_count,
        transition_file,
        worker_name: worker.and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        }),
        can_run: issues.is_empty(),
        issues,
        outputs,
    }
}

#[tauri::command]
// opens the slinghub page with the system browser
fn open_sling() -> Result<(), String> {
    const SLING_URL: &str = "https://sling.sg/";

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler").arg(SLING_URL);
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
        command
    };

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(SLING_URL);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(SLING_URL);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("the SLING website could not be opened: {error}"))
}

#[tauri::command]
// returns transition names for the selector
fn visualizer_list_transitions(project_path: String) -> Result<Vec<String>, String> {
    let project = PathBuf::from(project_path);
    let first = plot_files(&project)?
        .into_iter()
        .next()
        .ok_or_else(|| "no RFkit plot data was found in misc".to_string())?;
    transition_names(&first)
}

#[tauri::command]
// reads one transition from every plot binary
fn visualizer_transition(
    project_path: String,
    transition: String,
) -> Result<VisualizerTransitionData, String> {
    if transition.trim().is_empty() {
        return Err("select a transition before rendering".to_string());
    }
    let project = PathBuf::from(project_path);
    let mut samples = Vec::new();
    let mut well_count = 0;
    let mut global_rt_min = f32::INFINITY;
    let mut global_rt_max = f32::NEG_INFINITY;
    let mut global_intensity_max = 0.0_f32;
    let mut transition_seen = false;
    let plates = plate_by_sequence(&project);
    let sip_order = sip_order_by_well(&project);

    for path in plot_files(&project)? {
        match parse_selected_transition(&path, &transition, &plates, &sip_order)? {
            Some(sample) => {
                transition_seen = true;
                if !sample.wells.is_empty() {
                    for point in &sample.points {
                        global_rt_min = global_rt_min.min(point.rt);
                        global_rt_max = global_rt_max.max(point.rt);
                        global_intensity_max = global_intensity_max.max(point.intensity);
                    }
                    well_count += sample.wells.len();
                    samples.push(sample);
                }
            }
            None => {}
        }
    }

    if samples.is_empty() {
        let message = if transition_seen {
            format!("transition {transition} has no nonblank well plots")
        } else {
            format!("transition {transition} was not found in RFkit plot data")
        };
        return Err(message);
    }
    if !global_rt_min.is_finite() || !global_rt_max.is_finite() {
        global_rt_min = 0.0;
        global_rt_max = 1.0;
    }

    Ok(VisualizerTransitionData {
        transition,
        sample_count: samples.len(),
        well_count,
        samples,
        global_rt_min,
        global_rt_max,
        global_intensity_max,
    })
}

#[tauri::command]
// lists saved batch.rftime versions
fn visualizer_list_bounds_backups(
    app: AppHandle,
    project_path: String,
) -> Result<BackupList, String> {
    let project = PathBuf::from(&project_path);
    if !project.is_dir() {
        return Err("the selected dataset folder does not exist".to_string());
    }
    ensure_original_bounds(&project)?;
    let directory = project.join(BOUNDS_BACKUP_DIR);
    let backups: Vec<String> = if directory.is_dir() {
        fs::read_dir(&directory)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "rftime")
            })
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect()
    } else {
        Vec::new()
    };
    let backups = order_backups(backups);
    let settings = read_settings(&app).unwrap_or_default();
    let last = settings
        .last_backups
        .get(&project_path)
        .cloned()
        .filter(|name| backups.contains(name));
    let labels = settings
        .backup_labels
        .get(&project_path)
        .map(|labels| {
            labels
                .iter()
                .filter(|(name, _)| backups.contains(name))
                .map(|(name, label)| (name.clone(), label.clone()))
                .collect()
        })
        .unwrap_or_default();
    Ok(BackupList {
        backups,
        last,
        labels,
    })
}

#[tauri::command]
// renames the visible backup label
fn visualizer_rename_bounds_backup(
    app: AppHandle,
    project_path: String,
    name: String,
    label: String,
) -> Result<(), String> {
    let project = PathBuf::from(&project_path);
    if !project.is_dir() {
        return Err("the selected dataset folder does not exist".to_string());
    }
    let name = safe_backup_name(&name)?.to_string();
    if name == ORIGINAL_BOUNDS_BACKUP {
        return Err("Original batch.rftime cannot be renamed".to_string());
    }
    if !project.join(BOUNDS_BACKUP_DIR).join(&name).is_file() {
        return Err("that backup version no longer exists".to_string());
    }
    let label = safe_backup_label(&label)?;
    let mut settings = read_settings(&app).unwrap_or_default();
    settings
        .backup_labels
        .entry(project_path)
        .or_default()
        .insert(name, label);
    write_settings(&app, &settings)
}

#[tauri::command]
// removes a saved backup file
fn visualizer_delete_bounds_backup(
    app: AppHandle,
    project_path: String,
    name: String,
) -> Result<(), String> {
    let project = PathBuf::from(&project_path);
    if !project.is_dir() {
        return Err("the selected dataset folder does not exist".to_string());
    }
    let name = safe_backup_name(&name)?.to_string();
    if name == ORIGINAL_BOUNDS_BACKUP {
        return Err("Original batch.rftime cannot be deleted".to_string());
    }
    let source = project.join(BOUNDS_BACKUP_DIR).join(&name);
    if !source.is_file() {
        return Err("that backup version no longer exists".to_string());
    }
    fs::remove_file(source).map_err(|error| error.to_string())?;
    let mut settings = read_settings(&app).unwrap_or_default();
    if settings
        .last_backups
        .get(&project_path)
        .is_some_and(|last| last == &name)
    {
        settings.last_backups.remove(&project_path);
    }
    let remove_project_labels = if let Some(labels) = settings.backup_labels.get_mut(&project_path)
    {
        labels.remove(&name);
        labels.is_empty()
    } else {
        false
    };
    if remove_project_labels {
        settings.backup_labels.remove(&project_path);
    }
    write_settings(&app, &settings)
}

// reruns RFkit after batch.rftime changes
fn rerun_after_bounds_change(app: AppHandle, project: PathBuf) -> Result<ProjectSummary, String> {
    run_worker(app, project)
}

#[tauri::command]
// restores a backup and refreshes plots
async fn visualizer_restore_bounds_backup(
    app: AppHandle,
    state: State<'_, RunState>,
    project_path: String,
    name: String,
) -> Result<SaveBoundsResult, String> {
    let project = PathBuf::from(&project_path);
    if !project.is_dir() {
        return Err("the selected dataset folder does not exist".to_string());
    }
    if state
        .0
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("RFkit is already running".to_string());
    }
    emit_status(&app, "running");
    let name = safe_backup_name(&name)?.to_string();
    let restore_name = name.clone();
    let worker_app = app.clone();
    let worker_project = project.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let source = worker_project.join(BOUNDS_BACKUP_DIR).join(&restore_name);
        if !source.is_file() {
            return Err("that backup version no longer exists".to_string());
        }
        fs::copy(&source, worker_project.join("batch.rftime"))
            .map_err(|error| error.to_string())?;
        rerun_after_bounds_change(worker_app, worker_project)
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result)
    .map(|project| SaveBoundsResult {
        written: 0,
        backup: Some(name.clone()),
        project,
    });

    state.0.store(false, Ordering::Release);
    emit_status(&app, if result.is_ok() { "complete" } else { "failed" });
    if result.is_ok() {
        let mut settings = read_settings(&app).unwrap_or_default();
        settings.last_backups.insert(project_path, name);
        let _ = write_settings(&app, &settings);
    }
    result
}

#[tauri::command]
// saves edited bounds and refreshes plots
async fn visualizer_save_bounds(
    app: AppHandle,
    state: State<'_, RunState>,
    project_path: String,
    edits: Vec<BoundEdit>,
    backup_name: String,
) -> Result<SaveBoundsResult, String> {
    if edits.is_empty() {
        return Err("no integration bounds have changed".to_string());
    }
    let project = PathBuf::from(&project_path);
    if !project.is_dir() {
        return Err("the selected dataset folder does not exist".to_string());
    }
    if state
        .0
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("RFkit is already running".to_string());
    }

    emit_status(&app, "running");
    let name = safe_backup_name(&backup_name)?.to_string();
    let worker_app = app.clone();
    let worker_project = project.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        ensure_original_bounds(&worker_project)?;
        let written = save_bounds_to_batch(&worker_project, &edits)?;
        let backup = backup_bounds(&worker_project, &name)?;
        let summary = rerun_after_bounds_change(worker_app, worker_project)?;
        Ok(SaveBoundsResult {
            written,
            backup,
            project: summary,
        })
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result);

    state.0.store(false, Ordering::Release);
    emit_status(&app, if result.is_ok() { "complete" } else { "failed" });
    if let Ok(result) = &result
        && let Some(backup) = &result.backup
    {
        let mut settings = read_settings(&app).unwrap_or_default();
        settings.last_backups.insert(project_path, backup.clone());
        let _ = write_settings(&app, &settings);
    }
    result
}

#[tauri::command]
// loads remembered project state on startup
fn load_startup_state(app: AppHandle) -> StartupState {
    let settings = read_settings(&app);
    let remembered = settings.as_ref().and_then(|settings| {
        (!settings.last_project.is_empty()).then(|| PathBuf::from(&settings.last_project))
    });
    let project = remembered
        .as_deref()
        .filter(|path| path.is_dir())
        .map(inspect_project);
    StartupState {
        remembered_path: remembered
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        needs_reselection: remembered.is_some() && project.is_none(),
        project,
        theme: settings.and_then(|settings| settings.theme),
    }
}

#[tauri::command]
// selects and remembers a project folder
fn select_project(app: AppHandle, path: String) -> Result<ProjectSummary, String> {
    let project = PathBuf::from(path);
    if !project.is_dir() {
        return Err("the selected project folder does not exist".to_string());
    }
    save_project(&app, &project)?;
    Ok(inspect_project(&project))
}

#[tauri::command]
// refreshes project state without changing settings
fn refresh_project(path: String) -> Result<ProjectSummary, String> {
    let project = PathBuf::from(path);
    if !project.is_dir() {
        return Err("the selected project folder does not exist".to_string());
    }
    Ok(inspect_project(&project))
}

#[tauri::command]
// saves the preferred theme
fn set_theme(app: AppHandle, theme: String) -> Result<(), String> {
    if theme != "light" && theme != "dark" {
        return Err("the interface theme must be light or dark".to_string());
    }
    let mut settings = read_settings(&app).unwrap_or_default();
    settings.theme = Some(theme);
    write_settings(&app, &settings)
}

// streams worker output back to the ui
fn forward_output<R: Read>(
    reader: R,
    app: &AppHandle,
    stream: &str,
    prefix: &str,
) -> Result<(), String> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' || byte[0] == b'\r' => {
                if !line.is_empty() {
                    emit_output(app, stream, prefix, &line);
                    line.clear();
                }
            }
            Ok(_) => line.push(byte[0]),
            Err(error) => return Err(error.to_string()),
        }
    }
    if !line.is_empty() {
        emit_output(app, stream, prefix, &line);
    }
    Ok(())
}

// emits one worker log line
fn emit_output(app: &AppHandle, stream: &str, prefix: &str, line: &[u8]) {
    let text = String::from_utf8_lossy(line).trim().to_string();
    if text.is_empty() {
        return;
    }
    let _ = app.emit(
        "worker-output",
        WorkerOutput {
            stream: stream.to_string(),
            line: format!("{prefix}: {text}"),
        },
    );
}

// emits worker running state
fn emit_status(app: &AppHandle, status: &str) {
    let _ = app.emit(
        "run-state",
        RunStatus {
            status: status.to_string(),
        },
    );
}

// runs one RFkit command step
fn run_worker_command(
    app: &AppHandle,
    project: &Path,
    worker: &Path,
    arg: &str,
    label: &str,
) -> Result<(), String> {
    let _ = app.emit(
        "worker-output",
        WorkerOutput {
            stream: "output".to_string(),
            line: format!("Starting {label}."),
        },
    );

    let mut command = Command::new(worker);
    command
        .arg(arg)
        .current_dir(project)
        .env("RFKIT_PROJECT_DIR", project)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "the processing output stream was unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "the processing error stream was unavailable".to_string())?;

    let stderr_app = app.clone();
    let stderr_label = label.to_string();
    let stderr_thread =
        std::thread::spawn(move || forward_output(stderr, &stderr_app, "error", &stderr_label));
    forward_output(stdout, app, "output", label)?;
    let status = child.wait().map_err(|error| error.to_string())?;
    stderr_thread
        .join()
        .map_err(|_| "the processing error reader stopped unexpectedly".to_string())??;

    if !status.success() {
        return Err(format!(
            "{label} exited with code {}",
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

// runs acquisition timing and gui plot data steps
fn run_worker(app: AppHandle, project: PathBuf) -> Result<ProjectSummary, String> {
    let summary = inspect_project(&project);
    if !summary.can_run {
        return Err(summary.issues.join("; "));
    }
    let worker = find_worker(&project)
        .ok_or_else(|| "the RFkit processing engine was not found".to_string())?;

    run_worker_command(&app, &project, &worker, "1", "acquisition time generation")?;
    run_worker_command(
        &app,
        &project,
        &worker,
        "2only",
        "area calculation and gui plot data",
    )?;
    Ok(inspect_project(&project))
}

#[tauri::command]
// runs RFkit while preventing duplicate runs
async fn run_all(
    app: AppHandle,
    state: State<'_, RunState>,
    path: String,
) -> Result<ProjectSummary, String> {
    let project = PathBuf::from(path);
    if !project.is_dir() {
        return Err("the selected project folder does not exist".to_string());
    }
    if state
        .0
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("RFkit is already running".to_string());
    }

    emit_status(&app, "running");
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || run_worker(worker_app, project))
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result);

    state.0.store(false, Ordering::Release);
    emit_status(&app, if result.is_ok() { "complete" } else { "failed" });
    result
}

fn main() {
    tauri::Builder::default()
        .manage(RunState(AtomicBool::new(false)))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            load_startup_state,
            select_project,
            refresh_project,
            set_theme,
            open_sling,
            visualizer_list_transitions,
            visualizer_transition,
            visualizer_list_bounds_backups,
            visualizer_rename_bounds_backup,
            visualizer_delete_bounds_backup,
            visualizer_restore_bounds_backup,
            visualizer_save_bounds,
            run_all
        ])
        .run(tauri::generate_context!())
        .expect("error while running RFkit GUI");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_name_uses_platform_extension() {
        if cfg!(windows) {
            assert_eq!(worker_name("RFkit"), "RFkit.exe");
        } else {
            assert_eq!(worker_name("RFkit"), "RFkit");
        }
    }

    #[test]
    fn transition_counter_ignores_header_and_comments() {
        let path = std::env::temp_dir().join(format!(
            "rfkit_transition_count_{}_{}.csv",
            std::process::id(),
            "test"
        ));
        fs::write(&path, "# note\nID,q1,q3\nA,1,2\n\nB,3,4\n").unwrap();
        assert_eq!(count_transition_rows(&path), 2);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn plate_labels_are_read_from_space_aligned_batch_file() {
        let project = std::env::temp_dir().join(format!(
            "rfkit_plate_labels_{}_{}",
            std::process::id(),
            "test"
        ));
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("batch.rftime"),
            "plate     sip    seq    row     col     siptime     sipsensor\n------------------------------------------------------------------------\nPlate1 1 1 1 1 3.073 0\nPlate2 1 2 1 1 3.073 0\n",
        )
        .unwrap();
        let plates = plate_by_sequence(&project);
        assert_eq!(sample_display_name("sequence1.mzML", &plates), "plate 1");
        assert_eq!(sample_display_name("sequence2.mzML", &plates), "plate 2");
        fs::remove_dir_all(project).unwrap();
    }

    #[test]
    fn tabbed_batch_rows_keep_spaced_plate_names() {
        let project = std::env::temp_dir().join(format!(
            "rfkit_spaced_plate_labels_{}_{}",
            std::process::id(),
            "test"
        ));
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("batch.rftime"),
            "plate     sip    seq    row     col     siptime     sipsensor\n------------------------------------------------------------------------\nPlate 1 EMD\t1\t1\t2\t1\t3.809\t1\nPlate 1 EMD\t2\t1\t3001\t3\t15.095\t1\n",
        )
        .unwrap();

        let plates = plate_by_sequence(&project);
        assert_eq!(plates.get(&1).map(String::as_str), Some("Plate 1 EMD"));
        assert_eq!(sample_display_name("sequence1.mzML", &plates), "plate 1");

        let sip_order = sip_order_by_well(&project);
        assert_eq!(sip_order.get(&(1, 2, 1)).copied(), Some(1));
        assert_eq!(sip_order.len(), 1);
        fs::remove_dir_all(project).unwrap();
    }

    #[test]
    fn plot_files_sort_by_numeric_sequence() {
        let project =
            std::env::temp_dir().join(format!("rfkit_plot_sort_{}_{}", std::process::id(), "test"));
        let misc = project.join("misc");
        fs::create_dir_all(&misc).unwrap();
        fs::write(misc.join("plot_sequence10.mzML.bin"), []).unwrap();
        fs::write(misc.join("plot_sequence2.mzML.bin"), []).unwrap();
        fs::write(misc.join("plot_sequence1.mzML.bin"), []).unwrap();
        let names: Vec<String> = plot_files(&project)
            .unwrap()
            .into_iter()
            .map(|path| sample_id_from_plot(&path))
            .collect();
        assert_eq!(
            names,
            vec!["sequence1.mzML", "sequence2.mzML", "sequence10.mzML"]
        );
        fs::remove_dir_all(project).unwrap();
    }
}
