//! Small shared helpers used across subcommands.

use std::fs;
use std::path::Path;

/// Return the value following `flag` in `args`, if present.
pub(crate) fn arg_value(args: &[String], flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).cloned()
}

/// Write `contents` to `path`, creating any missing parent directories.
pub(crate) fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    fs::write(path, contents).map_err(|e| format!("writing {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_value_finds_and_misses() {
        let a: Vec<String> = ["run", "--dest", "/mnt/sd", "--release"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(arg_value(&a, "--dest"), Some("/mnt/sd".to_string()));
        assert_eq!(arg_value(&a, "--missing"), None);
    }

    #[test]
    fn arg_value_flag_without_value_is_none() {
        let a = vec!["--dest".to_string()];
        assert_eq!(arg_value(&a, "--dest"), None);
    }
}
