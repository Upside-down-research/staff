use std::io::Result;
fn main() -> Result<()> {
    // This is largely obviated by the proto prost gen plugin (the _standard_ protoc way).
    // Enable to generate a staff.rs in the `target` output
    //prost_build::compile_protos(&["protos/staff.proto"], &["src/"])?;
    Ok(())
}
