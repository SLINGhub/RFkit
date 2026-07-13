mod parse;
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
const MISCDIR: &str = "misc";
pub fn calc_auc(param_t: &crate::common::Param) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir(MISCDIR).ok();
    let t_list = read_assay(&param_t.t_list).unwrap();
    let mzml_fs_acq_time = read_acq_time()?;
    let dat_l: Vec<_> = mzml_fs_acq_time
        .iter()
        .map(|x| parse_auc(x, &t_list, param_t))
        .collect::<Result<_, _>>()?;
    write_long(&dat_l, &mzml_fs_acq_time, &t_list)?;
    Ok(())
}
fn write_long(
    dat_l: &[Vec<Vec<RowDat>>],
    mzml_fs_acq_time: &[Acq],
    t_list: &[QQ],
) -> std::io::Result<()> {
    let mut wtr = csv::WriterBuilder::new().from_path("long.csv")?;
    wtr.write_record(["sample", "ID", "well", "RT at apex", "area", "height"])?;
    for (Acq { sam_id, well }, dat_l_f) in mzml_fs_acq_time.iter().zip(dat_l) {
        for (qq, dat_l_f_c) in t_list.iter().zip(dat_l_f) {
            for (dat, (well_id, _, _)) in dat_l_f_c.iter().zip(well) {
                wtr.write_record([
                    sam_id,
                    &qq.name,
                    well_id,
                    &format!("{:.3}", dat.rt),
                    &format!("{:.1}", dat.auc),
                    &format!("{:.1}", dat.height),
                ])?;
            }
        }
    }
    Ok(())
}

struct Acq {
    sam_id: String,
    well: Vec<(String, f32, f32)>,
}
fn parse_auc(
    Acq { sam_id, well }: &Acq,
    t_list: &[QQ],
    param_t: &crate::common::Param,
) -> Result<Vec<Vec<RowDat>>, Box<dyn Error>> {
    let &crate::common::Param { mz_tol, .. } = param_t;
    println!("{sam_id:?}");

    let file_path = Path::new(MISCDIR).join(format!("plot_{sam_id}.bin"));
    let mut bufp = BufWriter::new(File::create(file_path)?);

    let file_path = std::path::Path::new("mzml_dir").join(sam_id);
    let (_ts, qqeic) = parse::mzml(&file_path);
    bufp.write_all(&u8::try_from(t_list.len())?.to_le_bytes())?;
    t_list
        .iter()
        .map(|trans| print_plot(mz_tol, &mut bufp, trans, &qqeic, well))
        .collect::<Result<_, _>>()
}
fn find_closest(vec: &[(f32, f32)], pt: f32, pos: usize) -> usize {
    if pos == 0 {
        pos
    } else if pos == vec.len() || pt - vec[pos - 1].0 <= vec[pos].0 - pt {
        pos - 1
    } else {
        pos
    }
}
fn print_plot(
    mz_tol: f32,
    bufp: &mut BufWriter<File>,
    trans: &QQ,
    qqeic: &[parse::Q1Q3RtI],
    acq_time: &[(String, f32, f32)],
) -> Result<Vec<RowDat>, Box<dyn Error>> {
    let pos0 = trans.q1 - mz_tol;
    let pos1 = trans.q1 + mz_tol;
    let pos0 = qqeic.partition_point(|x| x.q1 < pos0);
    let rt_i_l = qqeic[pos0..]
        .iter()
        .take_while(|x| x.q1 < pos1)
        .map(|x| (x, (x.q3 - trans.q3).abs()))
        .filter(|x| x.1 < mz_tol)
        .min_by(|x, y| x.1.partial_cmp(&y.1).unwrap())
        .map_or_else(|| panic!("{} not found", trans.name), |x| &x.0.rt_i_l);
    bufp.write_all(trans.name.as_bytes())?;
    bufp.write_all(b"\0")?;
    bufp.write_all(&u16::try_from(rt_i_l.len())?.to_le_bytes())?;
    for x in rt_i_l {
        bufp.write_all(&x.0.to_le_bytes())?;
        bufp.write_all(&x.1.to_le_bytes())?;
    }
    bufp.write_all(&u8::try_from(acq_time.len())?.to_le_bytes())?;
    acq_time
        .iter()
        .map(|(lab, beg, end)| {
            bufp.write_all(lab.as_bytes())?;
            bufp.write_all(b"\0")?;
            let pos1 = rt_i_l.partition_point(|y| y.0 < *end);
            let pos0 = rt_i_l[..pos1].partition_point(|y| y.0 < *beg);
            let pos0 = find_closest(rt_i_l, *beg, pos0);
            bufp.write_all(&u16::try_from(pos0 + 1)?.to_le_bytes())?;
            let pos1 = 1 + find_closest(rt_i_l, *end, pos1);
            bufp.write_all(&u16::try_from(pos1)?.to_le_bytes())?;
            let auc = rt_i_l[pos0..pos1]
                .windows(2)
                .map(|x| (x[0].1 + x[1].1) * (x[1].0 - x[0].0))
                .sum::<f32>()
                * 30.;
            let (rt, height): (f32, f32) = rt_i_l[pos0..pos1]
                .iter()
                .max_by(|x, y| x.1.partial_cmp(&y.1).unwrap())
                .map_or((*beg, 0.), |x| *x);
            Ok(RowDat { auc, height, rt })
        })
        .collect()
}

struct RowDat {
    auc: f32,
    height: f32,
    rt: f32,
}

fn read_acq_time() -> std::io::Result<Vec<Acq>> {
    let rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .trim(csv::Trim::All)
        .from_path("acq_time.csv")?;
    let mut acq_time = Vec::new();
    let mut ptr = &mut Acq {
        sam_id: String::new(),
        well: Vec::new(),
    };
    for row in rdr.into_records().map(std::result::Result::unwrap) {
        if row[1].is_empty() && row[2].is_empty() {
            acq_time.push(Acq {
                sam_id: row[0].to_string(),
                well: Vec::with_capacity(99),
            });
            ptr = acq_time.last_mut().unwrap();
        } else {
            ptr.well.push((
                row[0].to_string(),
                row[1].parse().unwrap(),
                row[2].parse().unwrap(),
            ));
        }
    }
    Ok(acq_time)
}
struct QQ {
    name: String,
    q1: f32,
    q3: f32,
}
fn read_assay(assay_f: &Path) -> std::io::Result<Vec<QQ>> {
    let mut rdr = csv::ReaderBuilder::new()
        .comment(Some(b'#'))
        .has_headers(true)
        .trim(csv::Trim::All)
        .from_path(assay_f)?;
    let t_list: Vec<_> = rdr
        .records()
        .map(std::result::Result::unwrap)
        .map(|line| QQ {
            name: line[0].to_string(),
            q1: line[1].parse().unwrap(),
            q3: line[2].parse().unwrap(),
        })
        .collect();
    Ok(t_list)
}
