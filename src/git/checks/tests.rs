//! Tests on constructed outputs: they pin our side of the contract,
//! the interpreter tables. Git itself never runs here.

use super::*;

fn output(status: i32, stdout: &str, stderr: &str) -> GitOutput {
    GitOutput {
        command: String::from("git (test)"),
        status,
        signal: None,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    }
}

mod git_available {
    use super::*;

    #[test]
    fn reports_a_version() {
        let out = output(0, "git version 2.43.0\n", "");
        assert!(matches!(interpret_git_available(out), Ok(true)));
    }

    #[test]
    fn unrecognized_success_output() {
        let out = output(0, "surprise\n", "");
        assert!(matches!(
            interpret_git_available(out),
            Err(GitError::UnexpectedOutput { .. })
        ));
    }

    #[test]
    fn unrecognized_failure() {
        let out = output(1, "", "some error\n");
        assert!(matches!(
            interpret_git_available(out),
            Err(GitError::CommandFailed { .. })
        ));
    }

    fn spawn_failure(kind: std::io::ErrorKind) -> GitError {
        GitError::SpawnFailed {
            command: String::from("git (test)"),
            source: std::io::Error::new(kind, "spawn failed"),
        }
    }

    #[test]
    fn a_missing_binary_is_an_answer() {
        let error = spawn_failure(std::io::ErrorKind::NotFound);
        assert!(matches!(interpret_spawn_failure(error, false), Ok(false)));
    }

    #[test]
    fn a_non_executable_binary_is_an_answer() {
        let error = spawn_failure(std::io::ErrorKind::PermissionDenied);
        assert!(matches!(interpret_spawn_failure(error, false), Ok(false)));
    }

    #[test]
    fn blame_on_the_workdir_stays_an_error() {
        let error = spawn_failure(std::io::ErrorKind::NotFound);
        assert!(matches!(
            interpret_spawn_failure(error, true),
            Err(GitError::SpawnFailed { .. })
        ));
    }

    #[test]
    fn an_alien_spawn_failure_stays_an_error() {
        let error = spawn_failure(std::io::ErrorKind::OutOfMemory);
        assert!(matches!(
            interpret_spawn_failure(error, false),
            Err(GitError::SpawnFailed { .. })
        ));
    }
}

mod in_work_tree {
    use super::*;

    #[test]
    fn inside_a_work_tree() {
        let out = output(0, "true\n", "");
        assert!(matches!(interpret_in_work_tree(out), Ok(true)));
    }

    #[test]
    fn inside_the_git_dir() {
        let out = output(0, "false\n", "");
        assert!(matches!(interpret_in_work_tree(out), Ok(false)));
    }

    #[test]
    fn outside_any_repository() {
        let out = output(
            128,
            "",
            "fatal: not a git repository (or any of the parent directories): .git\n",
        );
        assert!(matches!(interpret_in_work_tree(out), Ok(false)));
    }

    /// Tracing variables (GIT_TRACE and friends) prepend chatter to
    /// stderr; the answer line must still be recognized.
    #[test]
    fn outside_any_repository_with_trace_chatter() {
        let out = output(
            128,
            "",
            "20:03:41.000000 git.c:463 trace: built-in: git rev-parse --is-inside-work-tree\n\
             fatal: not a git repository (or any of the parent directories): .git\n",
        );
        assert!(matches!(interpret_in_work_tree(out), Ok(false)));
    }

    /// The anchor is the start of a line, not a substring: the
    /// matching phrase quoted mid-line by some other fatal message
    /// must not smuggle in the enumerated answer.
    #[test]
    fn a_quoted_phrase_mid_line_stays_an_error() {
        let out = output(
            128,
            "",
            "fatal: cannot change to 'fatal: not a git repository': No such file or directory\n",
        );
        assert!(matches!(
            interpret_in_work_tree(out),
            Err(GitError::CommandFailed { .. })
        ));
    }

    #[test]
    fn unrecognized_success_output() {
        let out = output(0, "maybe\n", "");
        assert!(matches!(
            interpret_in_work_tree(out),
            Err(GitError::UnexpectedOutput { .. })
        ));
    }

    #[test]
    fn unrecognized_failure() {
        let out = output(
            128,
            "",
            "fatal: detected dubious ownership in repository at '/x'\n",
        );
        assert!(matches!(
            interpret_in_work_tree(out),
            Err(GitError::CommandFailed { .. })
        ));
    }
}

mod head_exists {
    use super::*;

    #[test]
    fn resolves_to_a_commit() {
        let out = output(0, "0f2a1c7\n", "");
        assert!(matches!(interpret_head_exists(out), Ok(true)));
    }

    #[test]
    fn unborn() {
        let out = output(1, "", "");
        assert!(matches!(interpret_head_exists(out), Ok(false)));
    }

    #[test]
    fn unrecognized_failure() {
        let out = output(
            128,
            "",
            "fatal: this operation must be run in a work tree\n",
        );
        assert!(matches!(
            interpret_head_exists(out),
            Err(GitError::CommandFailed { .. })
        ));
    }
}

mod head_detached {
    use super::*;

    #[test]
    fn detached() {
        let out = output(1, "", "");
        assert!(matches!(interpret_head_detached(out), Ok(true)));
    }

    #[test]
    fn on_a_branch() {
        let out = output(0, "refs/heads/main\n", "");
        assert!(matches!(interpret_head_detached(out), Ok(false)));
    }

    #[test]
    fn unrecognized_failure() {
        let out = output(128, "", "fatal: ref HEAD is not a symbolic ref\n");
        assert!(matches!(
            interpret_head_detached(out),
            Err(GitError::CommandFailed { .. })
        ));
    }
}

mod index_has_staged {
    use super::*;

    #[test]
    fn index_differs_from_head() {
        let out = output(1, "", "");
        assert!(matches!(interpret_index_has_staged(out), Ok(true)));
    }

    #[test]
    fn index_matches_head() {
        let out = output(0, "", "");
        assert!(matches!(interpret_index_has_staged(out), Ok(false)));
    }

    #[test]
    fn unrecognized_failure() {
        let out = output(129, "", "usage: git diff --cached\n");
        assert!(matches!(
            interpret_index_has_staged(out),
            Err(GitError::CommandFailed { .. })
        ));
    }
}

mod index_conflicted {
    use super::*;

    #[test]
    fn has_unmerged_entries() {
        let out = output(0, "100644 8a1f3b2 1\tsrc/main.rs\n", "");
        assert!(matches!(interpret_index_conflicted(out), Ok(true)));
    }

    #[test]
    fn no_unmerged_entries() {
        let out = output(0, "", "");
        assert!(matches!(interpret_index_conflicted(out), Ok(false)));
    }

    #[test]
    fn unrecognized_failure() {
        let out = output(128, "", "fatal: index file corrupt\n");
        assert!(matches!(
            interpret_index_conflicted(out),
            Err(GitError::CommandFailed { .. })
        ));
    }
}

mod work_tree_clean {
    use super::*;

    #[test]
    fn clean() {
        let out = output(0, "", "");
        assert!(matches!(interpret_work_tree_clean(out), Ok(true)));
    }

    #[test]
    fn has_changes() {
        let out = output(0, " M src/main.rs\n?? notes.txt\n", "");
        assert!(matches!(interpret_work_tree_clean(out), Ok(false)));
    }

    #[test]
    fn unrecognized_failure() {
        let out = output(128, "", "fatal: index file corrupt\n");
        assert!(matches!(
            interpret_work_tree_clean(out),
            Err(GitError::CommandFailed { .. })
        ));
    }
}

mod git_path {
    use super::*;

    #[test]
    fn resolves_a_path() {
        let out = output(0, ".git/MERGE_HEAD\n", "");
        assert_eq!(interpret_git_path(out).unwrap(), ".git/MERGE_HEAD");
    }

    #[test]
    fn empty_path_is_unexpected() {
        let out = output(0, "\n", "");
        assert!(matches!(
            interpret_git_path(out),
            Err(GitError::UnexpectedOutput { .. })
        ));
    }

    #[test]
    fn mangled_path_is_unexpected() {
        let out = output(0, ".git/w\u{FFFD}rktrees/x/MERGE_HEAD\n", "");
        assert!(matches!(
            interpret_git_path(out),
            Err(GitError::UnexpectedOutput { .. })
        ));
    }

    #[test]
    fn unrecognized_failure() {
        let out = output(128, "", "fatal: not a git repository\n");
        assert!(matches!(
            interpret_git_path(out),
            Err(GitError::CommandFailed { .. })
        ));
    }
}
