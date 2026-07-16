mod acq_time;
mod common;
mod get_auc;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args();
    args.next();
    if let Ok(project_dir) = std::env::var("RFKIT_PROJECT_DIR") {
        std::env::set_current_dir(project_dir)?;
    } else {
        std::env::set_current_dir(std::env::current_exe()?.parent().unwrap())?;
    }
    let param_t = common::read_param()?;
    match args.next().as_deref() {
        Some("1") => acq_time::read(&param_t),
        Some("2") => {
            get_auc::calc_auc(&param_t)?;
            gen_plots()?;
        }
        Some("2only") => get_auc::calc_auc(&param_t)?,
        Some("plot") => gen_plots()?,
        _ => {
            println!();
            println!("---------- RFkit ----------");
            loop {
                if let Err(x) = handle_input() {
                    use yansi::Paint;
                    println!("{}", format!("Error: {x}").white().on_red().bright());
                }
            }
        }
    }
    Ok(())
}
fn handle_input() -> Result<(), Box<dyn Error>> {
    println!(
        r#"
Enter number.
1: Generate "acq_time.csv",
2: Calculate areas, generate chromatograms in PDFs"#
    );
    let mut guess = String::new();
    std::io::stdin().read_line(&mut guess)?;
    let param_t = common::read_param()?;
    let start = std::time::Instant::now();
    match guess.trim() {
        "1" => acq_time::read(&param_t),
        "2" => {
            get_auc::calc_auc(&param_t)?;
            gen_plots()?;
        }
        _ => return Ok(()),
    }
    println!("----------Completed, {:.1?}----------", start.elapsed());
    Ok(())
}
fn gen_plots() -> Result<(), Box<dyn Error>> {
    use std::process::{Command, Stdio};
    let rscript = std::env::var_os("RFKIT_RSCRIPT").unwrap_or_else(|| "Rscript".into());
    let script = std::env::var_os("RFKIT_PLOT_SCRIPT").unwrap_or_else(|| "RFkit_plot.r".into());
    let status = Command::new(&rscript)
        .arg(script)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                std::io::Error::new(
                    error.kind(),
                    "Rscript was not found. Install R and make Rscript available on PATH, or set RFKIT_RSCRIPT to the full Rscript executable path.",
                )
            } else {
                error
            }
        })?;
    if !status.success() {
        return Err(format!("Rscript exited with code {}", status.code().unwrap_or(-1)).into());
    }
    Ok(())
}
