//! Embeds a Windows version resource, so the version and product details show in the
//! exe's file properties. FileVersion/ProductVersion come from the package version in
//! Cargo.toml, which the release workflow sets from the release tag.

fn main() -> std::io::Result<()> {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }
    winresource::WindowsResource::new()
        .set("ProductName", "claude-ssh")
        .set(
            "FileDescription",
            "claude-ssh - run Claude Code as another local user via SSH",
        )
        .set("OriginalFilename", "claude-ssh.exe")
        .set("InternalName", "claude-ssh")
        .set("LegalCopyright", "Copyright (c) 2026 Tony Roberts")
        .compile()
}
