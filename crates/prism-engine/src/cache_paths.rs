//! 运行期生成物缓存目录解析——与构建输入（`assets/`）严格区分。
//!
//! IBL 预卷积、BRDF LUT、着色器 / 管线缓存这类**可以随时删除、删了重算**的产物
//! 不能写进 `assets/`：那是只读的构建输入目录（装到 `Program Files` 后不可写，
//! Android 上更在 APK 内）。它们也不该占用用户配置目录（`%APPDATA%`，通常在系统盘）。
//!
//! 所有运行期缓存统一挂在**同一个根**下，按类别分一级子目录：
//!
//! ```text
//! <可执行文件目录>/cache/
//!     ibl/        IBL 预卷积 + BRDF LUT
//!     shader/     管线缓存（未来）
//!     gi/         GI / 辐射缓存（未来）
//! ```
//!
//! 目录随 `target/` 走，因此同一 workspace 内无论从哪个工作目录启动都落到同一处：
//!
//! - `cargo run` → `target/debug/cache/`
//! - `cargo test` → 测试二进制在 `target/debug/deps/`，向上归一层，与上面共用同一份
//! - `cargo build --release` → `target/release/cache/`
//!
//! 删 `target/` 即彻底清理，不需要额外的 GC 逻辑。
//!
//! 解析只在宿主初始化时做一次，下游只消费传进来的绝对路径
//! （`prism-render` 不依赖本 crate，靠注入）。

use std::path::{Path, PathBuf};

/// 缓存根目录名（可执行文件同目录下）。
pub const CACHE_ROOT_DIR_NAME: &str = "cache";

/// 测试二进制所在的子目录名：从这里上跳一层，与同 profile 的普通二进制共用缓存。
#[cfg(test)]
const TEST_BINARY_DIR_NAME: &str = "deps";

/// 运行期缓存的类别。每类一个一级子目录，互不干扰，可各自整目录删除。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKind {
    /// IBL 预卷积产物与 BRDF LUT。
    Ibl,
    /// 管线缓存（`VkPipelineCache` 落盘）。
    Shader,
    /// GI / 辐射缓存。
    Gi,
}

impl CacheKind {
    /// 子目录名。
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Ibl => "ibl",
            Self::Shader => "shader",
            Self::Gi => "gi",
        }
    }
}

/// 缓存根目录：`<可执行文件目录>/cache/`。
///
/// 返回 `None` 表示无法确定可执行文件目录，调用方据此关闭磁盘缓存、每次重算。
pub fn cache_root() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    Some(profile_dir(&exe_dir).join(CACHE_ROOT_DIR_NAME))
}

/// 某一类缓存的目录：`<可执行文件目录>/cache/<kind>/`。
///
/// 目录**不保证存在**，由写入方按需 `create_dir_all`。
pub fn cache_dir(kind: CacheKind) -> Option<PathBuf> {
    Some(cache_root()?.join(kind.dir_name()))
}

const TARGET_DIR_NAME: &str = "target";

/// Resolves Cargo's profile directory (`target/debug`, `target/release`) even when
/// test runners nest binaries under `deps/<hash>/` or `build/<crate>/<hash>/out/`.
fn profile_dir(exe_dir: &Path) -> PathBuf {
    let mut current = Some(exe_dir);
    while let Some(dir) = current {
        if dir.file_name().and_then(|name| name.to_str()) == Some(TARGET_DIR_NAME) {
            return exe_dir
                .strip_prefix(dir)
                .ok()
                .and_then(|relative| relative.components().next())
                .map(|first| dir.join(first))
                .unwrap_or_else(|| dir.to_path_buf());
        }
        current = dir.parent();
    }
    exe_dir.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_root_sits_in_the_profile_directory() {
        let root = cache_root().expect("executable directory is resolvable in tests");
        assert_eq!(
            root.file_name().and_then(|name| name.to_str()),
            Some(CACHE_ROOT_DIR_NAME)
        );
        let profile = root.parent().expect("profile dir");
        assert!(
            profile.ancestors().any(|dir| {
                dir.file_name().and_then(|name| name.to_str()) == Some(TARGET_DIR_NAME)
            }),
            "cache root {:?} is not under a target/ directory",
            root
        );
        assert_ne!(
            profile.file_name().and_then(|name| name.to_str()),
            Some(TEST_BINARY_DIR_NAME)
        );
    }

    #[test]
    fn every_kind_gets_its_own_subdirectory() {
        let root = cache_root().expect("cache root");
        for kind in [CacheKind::Ibl, CacheKind::Shader, CacheKind::Gi] {
            let dir = cache_dir(kind).expect("cache dir");
            assert_eq!(dir.parent(), Some(root.as_path()));
            assert_eq!(
                dir.file_name().and_then(|name| name.to_str()),
                Some(kind.dir_name())
            );
        }
    }
}
