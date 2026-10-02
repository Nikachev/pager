use super::*;

pub(super) fn build_version(repo_root: &std::path::Path) -> Result<BuildVersion, String> {
    let date = command_text(repo_root, "date", &["-u", "+%y %m %d%H%M%S"])?;
    let mut parts = date.split_whitespace();
    let year = parts.next().unwrap();
    let month = parts.next().unwrap();
    let timestamp = parts.next().unwrap();
    let branch = command_text(repo_root, "git", &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let main_branch = "main";
    let clean = command_text(repo_root, "git", &["status", "--porcelain"])?.is_empty();
    let base = if branch == main_branch {
        "HEAD".to_owned()
    } else {
        command_text(repo_root, "git", &["merge-base", "HEAD", main_branch])?
    };
    let commit_dates = command_text(repo_root, "git", &["log", "--format=%cs", &base])?;
    let prefix = format!("20{year}-{month}-");
    let ordinal = commit_dates
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .count() as u32;
    let numeric = year.parse::<u32>().unwrap() * 1_000_000
        + month.parse::<u32>().unwrap() * 10_000
        + ordinal.min(9_999);
    let release = branch == main_branch && clean;
    let base_display = format!("{year}.{month}.{ordinal}");
    Ok(BuildVersion {
        display: if release {
            base_display
        } else {
            format!("{base_display}-{timestamp}")
        },
        numeric,
    })
}

pub(super) fn assert_release_tree(root: &std::path::Path) -> Result<(), String> {
    if command_text(root, "git", &["rev-parse", "--abbrev-ref", "HEAD"])? != "main" {
        return Err("release builds require main".into());
    }
    if !command_text(root, "git", &["status", "--porcelain"])?.is_empty() {
        return Err("release builds require a clean working tree".into());
    }
    Ok(())
}

pub(super) fn selected_board() -> Result<String, String> {
    let board = env::var("PAGER_BOARD").unwrap_or_else(|_| "nice-nano-v2".to_owned());
    if !matches!(board.as_str(), "nice-nano-v2" | "xiao-nrf52840") {
        return Err("PAGER_BOARD must be nice-nano-v2 or xiao-nrf52840".into());
    }
    Ok(board)
}

/// Content identity of tracked and nonignored sources; keys and build artifacts
/// are deliberately excluded. Deleted tracked paths contribute a missing marker.
pub(super) fn source_fingerprint(root: &std::path::Path) -> Result<String, String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("git source inventory failed".into());
    }
    let mut paths: Vec<&[u8]> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .collect();
    paths.sort();
    paths.dedup();
    let mut digest = Sha256::new();
    for path in paths {
        let path = std::str::from_utf8(path).map_err(|_| "source path is not UTF-8")?;
        if ["keys/", "dist/", ".venv/", ".git/"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
        {
            continue;
        }
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path.as_bytes());
        match fs::read(root.join(path)) {
            Ok(bytes) => {
                digest.update([1]);
                digest.update(Sha256::digest(bytes));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => digest.update([0]),
            Err(error) => return Err(format!("source fingerprint {path}: {error}")),
        }
    }
    Ok(hex(&digest.finalize()))
}
