use clap::Parser;
use std::fs::File;
use std::path::PathBuf;

#[derive(Debug, Parser, Clone)]
#[command(name = "k2g", about = "Generate CNC operations from a KiCad PCB")]
pub struct CliArgs {
    #[arg(
        short = 'p',
        long = "pth",
        default_value_t = false,
        help = "Drill and route all requiring plating."
    )]
    pub pth: bool,

    #[arg(
        short = 'n',
        long = "npth",
        default_value_t = false,
        help = "Final drills and route and non-plated features"
    )]
    pub npth: bool,

    #[arg(
        short = 'l',
        long = "outline",
        default_value_t = false,
        help = "Route the PCB outiline"
    )]
    pub outline: bool,

    #[arg(
        short = 'a',
        long = "all",
        default_value_t = false,
        help = "Do all operations"
    )]
    pub all: bool,

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

    pub fn operations_label(&self) -> String {
        if self.all {
            return "all".to_string();
        }

        let mut ops = Vec::new();
        if self.pth {
            ops.push("pth");
        }
        if self.npth {
            ops.push("npth");
        }
        if self.outline {
            ops.push("outline");
        }

        if ops.is_empty() {
            "none".to_string()
        } else {
            ops.join(",")
        }
    }

    pub fn output_label(&self) -> String {
        match &self.output {
            Some(path) => path.display().to_string(),
            None => "stdout".to_string(),
        }
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
