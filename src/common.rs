use std::error::Error;
use std::path::PathBuf;

pub struct Param {
    pub t_list: PathBuf,
    pub mz_tol: f32,
    pub offset: f32,
}

pub fn read_param() -> Result<Param, Box<dyn Error>> {
    let param = std::fs::read_to_string("param.txt")?;
    let value = param.parse::<toml::Table>()?;
    Ok(Param {
        t_list: PathBuf::from(value["transition_list"].as_str().unwrap()),
        mz_tol: 0.06,
        offset: value["offset"].as_float().unwrap() as f32,
    })
}
