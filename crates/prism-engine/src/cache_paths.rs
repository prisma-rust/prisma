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

/// Cargo 构建目录名：缓存根按它定位，确保落在可丢弃的产物目录里。
const TARGET_DIR_NAME: &str = "target";

/// Cargo 放测试二进制的子目录名（仅测试断言用）。
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

/// Cargo 的 profile 目录（`target/debug`、`target/release`）。
///
/// 可执行文件不一定直接躺在 profile 目录里：普通二进制在 `target/debug`，
/// 测试二进制在 `target/debug/deps`，有些运行器还会再套一层哈希目录
/// （`target/debug/deps/<hash>/`）。这些位置都应该共用同一份缓存，所以向上找
/// 名为 `target` 的祖先，取它下面的第一段子目录名作为 profile。
///
/// 找不到 `target` 时退回可执行文件目录本身（安装后的发布包即属此列）。
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
        // 缓存必须落在 target/<profile>/ 下：可丢弃、随 cargo clean 一起消失。
        // 测试二进制的实际层级随 cargo 版本和运行器而变，所以只断言「某个祖先是
        // target」，不写死有几层。
        let profile = root.parent().expect("profile dir");
        assert!(
            profile.ancestors().any(|dir| {
                dir.file_name().and_then(|name| name.to_str()) == Some(TARGET_DIR_NAME)
            }),
            "cache root {:?} is not under a target/ directory",
            root
        );
        // 并且绝不能落在 deps/ 里——那是测试二进制目录，不是共享 profile。
        assert_ne!(
            profile.file_name().and_then(|name| name.to_str()),
            Some(TEST_BINARY_DIR_NAME)
        );
    }

    #[test]
    fn profile_dir_unwraps_nested_test_directories() {
        // cargo 会把测试二进制放在 deps/ 下，有的运行器再套一层哈希目录。
        let cases = [
            ("/w/target/debug", "/w/target/debug"),
            ("/w/target/debug/deps", "/w/target/debug"),
            ("/w/target/debug/deps/ef4c05d3ab9ba736", "/w/target/debug"),
            ("/w/target/release", "/w/target/release"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                profile_dir(Path::new(input)),
                PathBuf::from(expected),
                "profile_dir({input})"
            );
        }
        // 不在 target/ 下时原样返回（安装后的发布包走这条路）。
        assert_eq!(
            profile_dir(Path::new("/opt/game")),
            PathBuf::from("/opt/game")
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
