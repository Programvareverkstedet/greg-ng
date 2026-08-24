fn get_git_commit() -> Option<String> {
    let repo = git2::Repository::discover(".").ok()?;
    let head = repo.head().ok()?;
    let commit = head.peel_to_commit().ok()?;
    Some(commit.id().to_string())
}

fn get_git_commit_date() -> Option<String> {
    let repo = git2::Repository::discover(".").ok()?;
    let head = repo.head().ok()?;
    let commit = head.peel_to_commit().ok()?;
    format_git_time(commit.time())
}

fn format_git_time(git_time: git2::Time) -> Option<String> {
    let offset = time::UtcOffset::from_whole_seconds(git_time.offset_minutes() * 60).ok()?;
    let datetime = time::OffsetDateTime::from_unix_timestamp(git_time.seconds())
        .ok()?
        .to_offset(offset);
    let format = time::macros::format_description!("[year]-[month]-[day]");
    datetime.format(&format).ok()
}

fn get_git_dirty() -> Option<bool> {
    let repo = git2::Repository::discover(".").ok()?;
    let mut status_options = git2::StatusOptions::new();
    status_options
        .include_untracked(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut status_options)).ok()?;
    Some(!statuses.is_empty())
}

fn main() {
    let commit = option_env!("GIT_COMMIT")
        .map(|s| s.to_string())
        .or_else(get_git_commit)
        .unwrap_or_else(|| "unknown".to_string());

    let commit_date = option_env!("GIT_COMMIT_DATE")
        .map(|s| s.to_string())
        .or_else(get_git_commit_date)
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = option_env!("GIT_DIRTY")
        .map(|s| s == "true")
        .or_else(get_git_dirty)
        .unwrap_or(false);

    let build_profile = std::env::var("OUT_DIR")
        .unwrap_or_else(|_| "unknown".to_string())
        .split(std::path::MAIN_SEPARATOR)
        .nth_back(3)
        .unwrap_or("unknown")
        .to_string();

    let dependencies = build_info_build::build_script()
        .collect_runtime_dependencies(build_info_build::DependencyDepth::Depth(1))
        .build()
        .crate_info
        .dependencies
        .into_iter()
        .map(|dep| format!("{}: {}", dep.name, dep.version))
        .collect::<Vec<_>>()
        .join(";");

    println!("cargo:rustc-env=GIT_COMMIT={}", commit);
    println!("cargo:rustc-env=GIT_COMMIT_DATE={}", commit_date);
    println!("cargo:rustc-env=GIT_DIRTY={}", dirty);
    println!("cargo:rustc-env=BUILD_PROFILE={}", build_profile);
    println!("cargo:rustc-env=DEPENDENCY_LIST={}", dependencies);
}
