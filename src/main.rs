mod acq_time;
mod common;
mod get_auc;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args();
    args.next();
    std::env::set_current_dir(std::env::current_exe()?.parent().unwrap())?;
    let param_t = crate::common::read_param()?;
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
    Command::new("Rscript")
        .arg("RFkit_plot.r")
        .stdout(Stdio::inherit())
        .output()?;
    Ok(())
}
