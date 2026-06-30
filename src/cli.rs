use clap::Parser;
use std::fs::File;
use std::path::PathBuf;

#[derive(Debug, Parser, Clone)]
#[command(name = "k2g", about = "CAM software for KiCAD PCB production")]
pub struct CliArgs {
    #[arg(
        short = 'o',
        long = "output",
        value_name = "FILE",
        help = "Specify an output file name. Defaults to stdout"
    )]
    pub output: Option<PathBuf>,

    #[arg(value_name = "FILENAME", value_parser = validate_input_file)]
    pub filename: Option<PathBuf>,
}

impl CliArgs {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    pub fn save_filename_override(&self) -> Option<String> {
        self.filename.as_ref().map(|path| {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("job");
            format!("{stem}.nc")
        })
    }
}

fn validate_input_file(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);

    if !path.exists() {
        return Err(format!("input file does not exist: {}", path.display()));
    }

    if !path.is_file() {
        return Err(format!("input path is not a file: {}", path.display()));
    }

    File::open(&path)
        .map_err(|e| format!("input file is not readable: {} ({e})", path.display()))?;

    Ok(path)
}
