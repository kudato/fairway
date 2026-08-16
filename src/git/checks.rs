//! Fact checks about the state of the repository. The answer
//! contract lives in the doc of the parent module; one note for
//! maintainers: the bash reference matched the merged stream, so
//! judging on stdout alone is a deliberate departure.

#[cfg(test)]
mod tests;

use std::io::ErrorKind;
use std::path::Path;

use super::{Git, GitError, GitOutput};

/// The stable core of the message git prints when there is no
/// repository anywhere above the directory it ran in.
const NOT_A_REPOSITORY: &str = "fatal: not a git repository";

impl Git {
    /// Whether git resolves and executes. A missing or non-executable
    /// binary is an enumerated negative answer, not a failure.
    ///
    /// # Errors
    ///
    /// Unrecognized `--version` output, an unexpected exit status, or
    /// a spawn failure better explained by the working directory than
    /// by the binary.
    pub async fn git_available(&self) -> Result<bool, GitError> {
        match self.run(&["--version"]).await {
            Ok(out) => interpret_git_available(out),
            Err(e) => {
                // A broken workdir raises the same error kinds from
                // chdir as a missing binary does from exec; retrying
                // without the workdir pins the blame on one of them.
                let blamed_on_workdir = missing_binary_kind(&e)
                    && self.workdir.is_some()
                    && Git::new().run(&["--version"]).await.is_ok();
                interpret_spawn_failure(e, blamed_on_workdir)
            }
        }
    }

    /// Whether the working directory is inside a git work tree.
    /// A repository broken badly enough that git refuses it (say, a
    /// mangled HEAD) answers exactly like no repository at all and
    /// reads as `false` — parity with native git, not an oversight;
    /// telling corruption apart would be a separate integrity check.
    ///
    /// # Errors
    ///
    /// Anything but status 0 with `true`/`false` on stdout, or
    /// status 128 with the not-a-repository message.
    pub async fn in_work_tree(&self) -> Result<bool, GitError> {
        interpret_in_work_tree(self.run(&["rev-parse", "--is-inside-work-tree"]).await?)
    }

    /// Whether HEAD resolves to a commit (the branch is not unborn).
    ///
    /// # Errors
    ///
    /// Statuses other than 0 (a commit) and 1 (unborn).
    pub async fn head_exists(&self) -> Result<bool, GitError> {
        interpret_head_exists(self.run(&["rev-parse", "--verify", "-q", "HEAD"]).await?)
    }

    /// Whether HEAD is detached from any branch. An unborn branch is
    /// not detached: HEAD still points at it symbolically.
    ///
    /// # Errors
    ///
    /// Statuses other than 1 (detached) and 0 (on a branch).
    pub async fn head_detached(&self) -> Result<bool, GitError> {
        interpret_head_detached(self.run(&["symbolic-ref", "-q", "HEAD"]).await?)
    }

    /// Whether the index differs from HEAD; on an unborn HEAD, from
    /// the empty tree.
    ///
    /// # Errors
    ///
    /// Statuses other than 1 (differs) and 0 (matches).
    pub async fn index_has_staged(&self) -> Result<bool, GitError> {
        interpret_index_has_staged(
            self.run(&["diff", "--cached", "--quiet", "--ignore-submodules=none"])
                .await?,
        )
    }

    /// Whether the index has unmerged entries.
    ///
    /// # Errors
    ///
    /// Any non-zero status.
    pub async fn index_conflicted(&self) -> Result<bool, GitError> {
        interpret_index_conflicted(self.run(&["ls-files", "--unmerged"]).await?)
    }

    /// Whether nothing is staged, modified, or untracked.
    ///
    /// # Errors
    ///
    /// Any non-zero status.
    pub async fn work_tree_clean(&self) -> Result<bool, GitError> {
        interpret_work_tree_clean(
            self.run(&[
                "status",
                "--porcelain",
                "--untracked-files=normal",
                "--ignore-submodules=none",
            ])
            .await?,
        )
    }

    /// Whether a merge has started and not concluded.
    ///
    /// # Errors
    ///
    /// Failure to resolve the marker path.
    pub async fn merge_in_progress(&self) -> Result<bool, GitError> {
        self.marker("MERGE_HEAD").await
    }

    /// Whether a cherry-pick has started and not concluded.
    ///
    /// # Errors
    ///
    /// Failure to resolve the marker path.
    pub async fn cherry_pick_in_progress(&self) -> Result<bool, GitError> {
        self.marker("CHERRY_PICK_HEAD").await
    }

    /// Whether a revert has started and not concluded.
    ///
    /// # Errors
    ///
    /// Failure to resolve the marker path.
    pub async fn revert_in_progress(&self) -> Result<bool, GitError> {
        self.marker("REVERT_HEAD").await
    }

    /// Whether a bisect has started and not concluded.
    ///
    /// # Errors
    ///
    /// Failure to resolve the marker path.
    pub async fn bisect_in_progress(&self) -> Result<bool, GitError> {
        self.marker("BISECT_LOG").await
    }

    /// Whether git am has started and not concluded.
    ///
    /// # Errors
    ///
    /// Failure to resolve the marker path.
    pub async fn am_in_progress(&self) -> Result<bool, GitError> {
        self.marker("rebase-apply/applying").await
    }

    /// Whether a rebase has started and not concluded, in either
    /// backend: merge (rebase-merge) or apply (rebase-apply).
    ///
    /// # Errors
    ///
    /// Failure to resolve the marker paths.
    pub async fn rebase_in_progress(&self) -> Result<bool, GitError> {
        if self.marker("rebase-merge").await? {
            return Ok(true);
        }
        self.marker("rebase-apply/rebasing").await
    }

    /// Whether the file `name` exists inside the git dir; operations
    /// in progress are marked by such files. `rev-parse --git-path`
    /// resolves inside the git dir, so linked worktrees are handled.
    async fn marker(&self, name: &str) -> Result<bool, GitError> {
        let path = interpret_git_path(self.run(&["rev-parse", "--git-path", name]).await?)?;
        let path = Path::new(&path);
        let resolved = match &self.workdir {
            Some(dir) if path.is_relative() => dir.join(path),
            _ => path.to_path_buf(),
        };
        // Existence like bash [ -e ]: anything inaccessible reads as absent.
        Ok(tokio::fs::try_exists(resolved).await.unwrap_or(false))
    }
}

fn interpret_git_available(out: GitOutput) -> Result<bool, GitError> {
    if out.status == 0 && out.stdout.trim_end().starts_with("git version") {
        return Ok(true);
    }
    Err(out.into_error())
}

/// Whether a spawn failure is of the kind a missing or
/// non-executable binary produces. Not proof on its own: a broken
/// working directory raises the same kinds from chdir.
fn missing_binary_kind(error: &GitError) -> bool {
    matches!(
        error,
        GitError::SpawnFailed { source, .. }
            if matches!(
                source.kind(),
                ErrorKind::NotFound | ErrorKind::PermissionDenied
            )
    )
}

/// The spawn-failure rows of the `git_available` table: a missing
/// binary is the enumerated "no git" answer; anything else —
/// including a failure pinned on the workdir — stays an error.
fn interpret_spawn_failure(error: GitError, blamed_on_workdir: bool) -> Result<bool, GitError> {
    if missing_binary_kind(&error) && !blamed_on_workdir {
        Ok(false)
    } else {
        Err(error)
    }
}

fn interpret_in_work_tree(out: GitOutput) -> Result<bool, GitError> {
    match (out.status, out.stdout.trim_end()) {
        (0, "true") => return Ok(true),
        (0, "false") => return Ok(false),
        // Matched per line: variables like GIT_TRACE prepend chatter
        // to stderr without changing the answer. A path quoted inside
        // some other fatal message could smuggle in a matching line;
        // that corner is accepted as the price of trace tolerance.
        (128, _) if out.stderr.lines().any(|l| l.starts_with(NOT_A_REPOSITORY)) => {
            return Ok(false);
        }
        _ => {}
    }
    Err(out.into_error())
}

fn interpret_head_exists(out: GitOutput) -> Result<bool, GitError> {
    match out.status {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(out.into_error()),
    }
}

fn interpret_head_detached(out: GitOutput) -> Result<bool, GitError> {
    match out.status {
        1 => Ok(true),
        0 => Ok(false),
        _ => Err(out.into_error()),
    }
}

fn interpret_index_has_staged(out: GitOutput) -> Result<bool, GitError> {
    match out.status {
        1 => Ok(true),
        0 => Ok(false),
        _ => Err(out.into_error()),
    }
}

fn interpret_index_conflicted(out: GitOutput) -> Result<bool, GitError> {
    match out.status {
        0 => Ok(!out.stdout.trim_end().is_empty()),
        _ => Err(out.into_error()),
    }
}

fn interpret_work_tree_clean(out: GitOutput) -> Result<bool, GitError> {
    match out.status {
        0 => Ok(out.stdout.trim_end().is_empty()),
        _ => Err(out.into_error()),
    }
}

fn interpret_git_path(out: GitOutput) -> Result<String, GitError> {
    let len = out.stdout.trim_end().len();
    // A replacement character means the path did not survive UTF-8
    // decoding; probing the mangled path would misread the marker as
    // absent, so refuse loudly instead.
    if out.status == 0 && len > 0 && !out.stdout.contains('\u{FFFD}') {
        let mut path = out.stdout;
        path.truncate(len);
        Ok(path)
    } else {
        Err(out.into_error())
    }
}
