//! Packages an RP2040 ELF into a UF2 file: `uf2pack <elf> <uf2>`.
//!
//! The build script uses this instead of a Python helper so the GUI and the
//! script share one implementation of the block layout.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [elf_path, uf2_path] = args.as_slice() else {
        anyhow::bail!("用法：uf2pack <elf> <uf2>");
    };
    let elf = std::fs::read(elf_path)?;
    let uf2 = firmware_manage_tool::uf2::pack(&elf)?;
    std::fs::write(uf2_path, &uf2)?;
    println!(
        "{uf2_path}: {} blocks, {} bytes",
        firmware_manage_tool::uf2::blocks(&uf2),
        uf2.len()
    );
    Ok(())
}
