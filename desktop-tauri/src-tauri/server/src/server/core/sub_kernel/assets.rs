use super::KERNEL_VERSION;

pub(super) struct Asset {
    pub name: &'static str,
    pub archive_sha256: &'static str,
    pub binary_sha256: &'static str,
}

pub(super) fn current() -> Option<Asset> {
    let (name, archive_sha256, binary_sha256) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => (
            "mihomo-windows-amd64-v1.19.32.zip",
            "1ac84e795b5b915446e20139677fb6cc013a0778e876e8d4c96f994750b4c4a3",
            "beb9878924d7bd38c67176b441ab5e668cc378bfe751c8dc8fef3eb5aaf566d5",
        ),
        ("windows", "aarch64") => (
            "mihomo-windows-arm64-v1.19.32.zip",
            "7a4f6e58af9a120920935f2150cf642d945085fdb3e753314505b334234e1d1e",
            "7211487d8e4092cf6cf6fbbdbc10a10fbde462847e36fb8a43479b0e4cad4b33",
        ),
        ("macos", "x86_64") => (
            "mihomo-darwin-amd64-v1.19.32.gz",
            "b7de9fc46af2ca985ecfb302a6362170e04ff2023dee1e82addc54133e669bc9",
            "cf40caed85721f25f50779462ac14e706f370d9bd15933762b07cf51c0ff63ca",
        ),
        ("macos", "aarch64") => (
            "mihomo-darwin-arm64-v1.19.32.gz",
            "3312a6780652c622890fd4357c6a853bbf865464fd047ac7b7f52dab8de18652",
            "94a386ec0149080deadd86b1f667363bde3c70f7489dba3258e52c56fc9a6d66",
        ),
        ("linux", "x86_64") => (
            "mihomo-linux-amd64-v1.19.32.gz",
            "8451100836c9eda194331c2babfad490b2faf30cebc1a04b0e76fd8ac2d35d10",
            "3122d100e8177501776109f1a6253a694611627cf4d7c7ec82705855cf8626a8",
        ),
        ("linux", "aarch64") => (
            "mihomo-linux-arm64-v1.19.32.gz",
            "9dd862e28b46ff7d775f169cceebc28deccaa0a9e804237d421cd2571e0caba0",
            "c80df2ca5159567804da883401e0118908fc1155e05df2baf3dbd4d3ecfc3754",
        ),
        _ => return None,
    };
    Some(Asset { name, archive_sha256, binary_sha256 })
}

pub fn download_url() -> Option<String> {
    current().map(|asset| format!(
        "https://github.com/MetaCubeX/mihomo/releases/download/{KERNEL_VERSION}/{}", asset.name,
    ))
}

pub fn release_url() -> String {
    format!("https://github.com/MetaCubeX/mihomo/releases/tag/{KERNEL_VERSION}")
}
