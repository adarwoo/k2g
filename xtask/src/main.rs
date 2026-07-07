mod schema_doc;

use anyhow::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask should be inside workspace root")
        .to_path_buf();

    let schema_dir = root.join("resources").join("schemas");
    let output_file = root.join("docs").join("SCHEMAS.md");

    schema_doc::generate_schema_docs(&schema_dir, &output_file)?;

    println!("Generated {}", output_file.display());

    Ok(())
}