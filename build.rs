use std::io::Result;
fn main() -> Result<()> {
    // Enable to generate a staff.rs in the `target` output
    //prost_build::compile_protos(&["protos/staff.proto"], &["src/"])?;
    Ok(())
}
