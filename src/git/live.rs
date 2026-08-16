//! Tests against a live git in throwaway repositories.
//!
//! These pin git's side of the contract: the enumerated statuses,
//! messages, and marker files the checks rely on. Repositories are
//! built hermetically, so the inherited environment cannot leak the
//! setup git into a real repository.

use tempfile::TempDir;

use super::{Git, GitError};

/// A fresh repository; the directory is deleted when the handle drops.
/// The initial branch name is pinned so tests can reference it.
/// Identity and signing are pinned in the repository configuration:
/// the commands under test create commits too, and git's fallback of
/// inventing an identity from the account and hostname fails on
/// machines without a usable one, such as CI runners.
async fn init_repo() -> (TempDir, Git) {
    let dir = tempfile::tempdir().unwrap();
    let git = Git::hermetic_at(dir.path());
    setup(&git, &["init", "-q", "-b", "main"]).await;
    setup(&git, &["config", "user.name", "fairway tests"]).await;
    setup(&git, &["config", "user.email", "tests@fairway"]).await;
    setup(&git, &["config", "commit.gpgsign", "false"]).await;
    (dir, git)
}

/// Run a setup command that is expected to succeed.
async fn setup(git: &Git, args: &[&str]) {
    let out = git.run(args).await.unwrap();
    assert_eq!(out.status, 0, "setup failed: {out:?}");
}

/// Make a setup commit; the identity comes from [`init_repo`].
async fn commit(git: &Git, message: &str) {
    setup(git, &["commit", "-q", "--allow-empty", "-m", message]).await;
}

fn write(dir: &TempDir, name: &str, content: &str) {
    std::fs::write(dir.path().join(name), content).unwrap();
}

/// Two branches, `main` and `side`, that both changed `f.txt` away
/// from a common base: merging, cherry-picking, or rebasing one onto
/// the other conflicts. Leaves HEAD on `main`.
async fn conflicting_branches(dir: &TempDir, git: &Git) {
    write(dir, "f.txt", "base\n");
    setup(git, &["add", "f.txt"]).await;
    commit(git, "base").await;
    setup(git, &["checkout", "-q", "-b", "side"]).await;
    write(dir, "f.txt", "side\n");
    setup(git, &["add", "f.txt"]).await;
    commit(git, "side").await;
    setup(git, &["checkout", "-q", "main"]).await;
    write(dir, "f.txt", "ours\n");
    setup(git, &["add", "f.txt"]).await;
    commit(git, "ours").await;
}

#[tokio::test]
async fn git_is_available() {
    assert!(Git::new().git_available().await.unwrap());
}

#[tokio::test]
async fn git_available_blames_a_missing_workdir() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("gone");
    let result = Git::at(&missing).git_available().await;
    assert!(
        matches!(result, Err(GitError::SpawnFailed { .. })),
        "a missing workdir must not read as git being absent: {result:?}"
    );
}

#[tokio::test]
async fn no_repository_in_an_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let git = Git::hermetic_at(dir.path());
    assert!(!git.in_work_tree().await.unwrap());
}

/// Inside the git dir the answer is the enumerated `false`, not an
/// error: the one interpreter row otherwise pinned only by a
/// constructed fixture.
#[tokio::test]
async fn the_git_dir_is_not_a_work_tree() {
    let (dir, _git) = init_repo().await;
    let inside = Git::hermetic_at(dir.path().join(".git"));
    assert!(!inside.in_work_tree().await.unwrap());
}

#[tokio::test]
async fn unborn_repository_answers_every_check() {
    let (_dir, git) = init_repo().await;
    assert!(git.in_work_tree().await.unwrap());
    assert!(!git.head_exists().await.unwrap());
    assert!(!git.head_detached().await.unwrap());
    assert!(!git.index_has_staged().await.unwrap());
    assert!(!git.index_conflicted().await.unwrap());
    assert!(git.work_tree_clean().await.unwrap());
    assert!(!git.merge_in_progress().await.unwrap());
    assert!(!git.cherry_pick_in_progress().await.unwrap());
    assert!(!git.revert_in_progress().await.unwrap());
    assert!(!git.bisect_in_progress().await.unwrap());
    assert!(!git.am_in_progress().await.unwrap());
    assert!(!git.rebase_in_progress().await.unwrap());
}

#[tokio::test]
async fn unborn_repository_with_staged_changes() {
    let (dir, git) = init_repo().await;
    write(&dir, "f.txt", "one\n");
    setup(&git, &["add", "f.txt"]).await;
    assert!(!git.head_exists().await.unwrap());
    assert!(git.index_has_staged().await.unwrap());
}

#[tokio::test]
async fn committed_repository_is_calm() {
    let (_dir, git) = init_repo().await;
    commit(&git, "first").await;
    assert!(git.head_exists().await.unwrap());
    assert!(!git.head_detached().await.unwrap());
    assert!(git.work_tree_clean().await.unwrap());
}

#[tokio::test]
async fn detached_head() {
    let (_dir, git) = init_repo().await;
    commit(&git, "first").await;
    setup(&git, &["checkout", "-q", "--detach"]).await;
    assert!(git.head_detached().await.unwrap());
}

#[tokio::test]
async fn staged_changes() {
    let (dir, git) = init_repo().await;
    commit(&git, "first").await;
    write(&dir, "f.txt", "one\n");
    setup(&git, &["add", "f.txt"]).await;
    assert!(git.index_has_staged().await.unwrap());
    assert!(!git.work_tree_clean().await.unwrap());
}

#[tokio::test]
async fn untracked_file() {
    let (dir, git) = init_repo().await;
    commit(&git, "first").await;
    write(&dir, "stray.txt", "hello\n");
    assert!(!git.work_tree_clean().await.unwrap());
    assert!(!git.index_has_staged().await.unwrap());
}

/// The quotePath pin must reach the child: without it, git mangles
/// non-ASCII paths on the answer channel. Doubles as the pin that
/// [`GitOutput::command`] reports the real command line.
#[tokio::test]
async fn non_ascii_paths_arrive_unquoted() {
    let (dir, git) = init_repo().await;
    write(&dir, "имя.txt", "hello\n");
    let out = git.run(&["status", "--porcelain"]).await.unwrap();
    assert_eq!(out.status, 0, "status failed: {out:?}");
    assert!(
        out.stdout.contains("имя.txt"),
        "quotePath must be pinned off: {out:?}"
    );
    assert!(
        out.command.starts_with("git -c core.quotePath=false "),
        "the reported command must carry the pins: {}",
        out.command
    );
}

/// The locale pin must reach the child: message matching relies on
/// an untranslated git. A shell alias dumps the child environment.
#[cfg(unix)]
#[tokio::test]
async fn the_child_locale_is_pinned() {
    let out = Git::new()
        .run(&["-c", "alias.print-env=!env", "print-env"])
        .await
        .unwrap();
    assert_eq!(out.status, 0, "alias run failed: {out:?}");
    assert!(
        out.stdout.lines().any(|line| line == "LC_ALL=C"),
        "LC_ALL=C must be in the child environment: {out:?}"
    );
}

#[tokio::test]
async fn merge_conflict() {
    let (dir, git) = init_repo().await;
    conflicting_branches(&dir, &git).await;

    let merge = git.run(&["merge", "side"]).await.unwrap();
    assert_eq!(merge.status, 1, "merge should conflict: {merge:?}");
    assert!(git.merge_in_progress().await.unwrap());
    assert!(git.index_conflicted().await.unwrap());
    assert!(!git.work_tree_clean().await.unwrap());
}

#[tokio::test]
async fn cherry_pick_conflict() {
    let (dir, git) = init_repo().await;
    conflicting_branches(&dir, &git).await;

    let pick = git.run(&["cherry-pick", "side"]).await.unwrap();
    assert_eq!(pick.status, 1, "cherry-pick should conflict: {pick:?}");
    assert!(git.cherry_pick_in_progress().await.unwrap());
    assert!(git.index_conflicted().await.unwrap());
}

#[tokio::test]
async fn revert_conflict() {
    let (dir, git) = init_repo().await;
    for content in ["a\n", "b\n", "c\n"] {
        write(&dir, "f.txt", content);
        setup(&git, &["add", "f.txt"]).await;
        commit(&git, content.trim_end()).await;
    }

    let revert = git.run(&["revert", "--no-edit", "HEAD~1"]).await.unwrap();
    assert_eq!(revert.status, 1, "revert should conflict: {revert:?}");
    assert!(git.revert_in_progress().await.unwrap());
}

#[tokio::test]
async fn bisect_started() {
    let (_dir, git) = init_repo().await;
    commit(&git, "first").await;
    setup(&git, &["bisect", "start"]).await;
    assert!(git.bisect_in_progress().await.unwrap());
}

#[tokio::test]
async fn rebase_merge_backend_conflict() {
    let (dir, git) = init_repo().await;
    conflicting_branches(&dir, &git).await;
    setup(&git, &["checkout", "-q", "side"]).await;

    let rebase = git.run(&["rebase", "main"]).await.unwrap();
    assert_eq!(rebase.status, 1, "rebase should conflict: {rebase:?}");
    assert!(git.rebase_in_progress().await.unwrap());
    assert!(!git.am_in_progress().await.unwrap());
}

#[tokio::test]
async fn rebase_apply_backend_conflict() {
    let (dir, git) = init_repo().await;
    conflicting_branches(&dir, &git).await;
    setup(&git, &["checkout", "-q", "side"]).await;

    let rebase = git.run(&["rebase", "--apply", "main"]).await.unwrap();
    assert_eq!(rebase.status, 1, "rebase should conflict: {rebase:?}");
    assert!(git.rebase_in_progress().await.unwrap());
    assert!(!git.am_in_progress().await.unwrap());
}

/// A real conflicting `git am`: the patch from `side` cannot apply
/// onto `main`'s change of the same line, and the marker file the
/// check probes is left by git itself, not by the test.
#[tokio::test]
async fn am_conflict() {
    let (dir, git) = init_repo().await;
    conflicting_branches(&dir, &git).await;

    let patch = git
        .run(&["format-patch", "-1", "side", "-o", "patches"])
        .await
        .unwrap();
    assert_eq!(patch.status, 0, "format-patch failed: {patch:?}");
    let am = git.run(&["am", patch.stdout.trim_end()]).await.unwrap();
    assert_eq!(am.status, 128, "am should conflict: {am:?}");
    assert!(git.am_in_progress().await.unwrap());
    assert!(!git.rebase_in_progress().await.unwrap());
}

/// No real operation leaves both `applying` and `rebasing` behind, so
/// the distinction between the two is pinned on hand-placed files.
#[tokio::test]
async fn am_is_not_rebase() {
    let (dir, git) = init_repo().await;
    std::fs::create_dir(dir.path().join(".git/rebase-apply")).unwrap();
    write(&dir, ".git/rebase-apply/applying", "");
    assert!(git.am_in_progress().await.unwrap());
    assert!(!git.rebase_in_progress().await.unwrap());

    write(&dir, ".git/rebase-apply/rebasing", "");
    assert!(git.rebase_in_progress().await.unwrap());
}

#[tokio::test]
async fn linked_worktree_markers() {
    let (dir, git) = init_repo().await;
    commit(&git, "first").await;
    setup(&git, &["worktree", "add", "-q", "linked"]).await;
    let linked = Git::hermetic_at(dir.path().join("linked"));

    assert!(!linked.cherry_pick_in_progress().await.unwrap());
    // The per-worktree git dir lives inside the main .git; from the
    // linked worktree, --git-path answers with an absolute path.
    write(&dir, ".git/worktrees/linked/CHERRY_PICK_HEAD", "");
    assert!(linked.cherry_pick_in_progress().await.unwrap());
    // The main worktree does not see the linked worktree's state.
    assert!(!git.cherry_pick_in_progress().await.unwrap());
}
