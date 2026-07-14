use std::error::Error;
use std::io::{BufRead, BufReader};
type AcqTime = (u8, u8, f32, f32);
pub fn read(param_t: &crate::common::Param) {
    let rt_l = read_log().unwrap();
    write_rt(&rt_l, param_t.offset).unwrap();
}
fn write_rt(rt_l: &[(u8, Vec<AcqTime>)], offset: f32) -> Result<(), Box<dyn Error>> {
    let tf = |x: f32| -> String { format!("{:.3}", x / 60. + offset) };
    let mut wtr = csv::WriterBuilder::new().from_path("acq_time.csv")?;
    for (seqn, rt_mzml) in rt_l {
        wtr.write_record([&format!("sequence{seqn}.mzML"), "", ""])?;
        for (row, col, sta, end) in rt_mzml {
            wtr.write_record([format!("({row}, {col})"), tf(*sta), tf(*end)])?;
        }
    }
    Ok(())
}
fn read_log() -> Result<Vec<(u8, Vec<AcqTime>)>, Box<dyn Error>> {
    let mut reader = BufReader::new(std::fs::File::open("batch.rftime")?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    line.clear();
    reader.read_line(&mut line)?;
    let mut start_end = Vec::new();
    let mut seq: u8 = 0;
    let mut ptr = &mut (0, Vec::new());
    let mut s_time = -1.;
    let mut row = 0;
    let mut col = 0;
    while reader.read_line(&mut line)? != 0 {
        let line_sp: Vec<&str> = line.trim_end().split('\t').collect();
        if line_sp[2].parse::<u8>()? != seq {
            seq = line_sp[2].parse()?;
            start_end.push((seq, Vec::new()));
            ptr = start_end.last_mut().unwrap();
        }
        if line_sp[3] == "3001" {
            ptr.1.push((row, col, s_time, line_sp[5].parse()?));
        } else {
            row = line_sp[3].parse()?;
            col = line_sp[4].parse()?;
            s_time = line_sp[5].parse()?;
        }
        line.clear();
    }
    Ok(start_end)
}
