use super::*;

fn manager(mode: PermissionMode) -> TerminalManager {
    TerminalManager::new(TerminalManagerOptions {
        cwd: PathBuf::from("/tmp"),
        permission_mode: mode,
        non_interactive_policy: NonInteractivePermissionPolicy::Deny,
        handler: None,
        kill_grace: Some(Duration::from_millis(300)),
    })
}

#[test]
fn create_and_capture_output_of_real_command() {
    smol::block_on(async {
        let manager = manager(PermissionMode::ApproveAll);
        let created = manager
            .create_terminal(
                CreateTerminalRequest::new("s1", "echo").args(vec!["hello".to_string()]),
            )
            .await
            .unwrap();

        let exit = manager
            .wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                "s1",
                created.terminal_id.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(exit.exit_status.exit_code, Some(0));

        // The reader task may still be draining the pipe's last buffered
        // bytes for an instant after `wait_for_terminal_exit` observes the
        // child has exited; give it a beat before asserting.
        smol::Timer::after(Duration::from_millis(50)).await;
        let output = manager
            .terminal_output(TerminalOutputRequest::new("s1", created.terminal_id))
            .await
            .unwrap();
        assert_eq!(output.output.trim(), "hello");
        assert!(!output.truncated);
    });
}

#[test]
fn output_truncates_at_configured_byte_limit() {
    smol::block_on(async {
        let manager = manager(PermissionMode::ApproveAll);
        let created = manager
            .create_terminal(
                CreateTerminalRequest::new("s1", "yes x | head -c 200").output_byte_limit(8u64),
            )
            .await
            .unwrap();

        manager
            .wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                "s1",
                created.terminal_id.clone(),
            ))
            .await
            .unwrap();
        smol::Timer::after(Duration::from_millis(50)).await;
        let output = manager
            .terminal_output(TerminalOutputRequest::new("s1", created.terminal_id))
            .await
            .unwrap();
        assert!(output.truncated);
        assert!(output.output.len() <= 8);
    });
}

#[test]
fn kill_terminates_the_process_group() {
    smol::block_on(async {
        let manager = manager(PermissionMode::ApproveAll);
        let created = manager
            .create_terminal(CreateTerminalRequest::new("s1", "sleep").args(vec!["30".to_string()]))
            .await
            .unwrap();
        let pid = manager.terminal_pid(&created.terminal_id);
        assert!(crate::platform::is_process_alive(pid));

        manager
            .kill_terminal(KillTerminalRequest::new("s1", created.terminal_id))
            .await
            .unwrap();
        assert!(!crate::platform::is_process_alive(pid));
    });
}

#[test]
fn create_terminal_denied_in_deny_all_mode() {
    smol::block_on(async {
        let manager = manager(PermissionMode::DenyAll);
        let result = manager
            .create_terminal(CreateTerminalRequest::new("s1", "echo"))
            .await;
        assert!(matches!(result, Err(AcpError::PermissionDenied(_))));
    });
}

#[test]
fn release_terminal_is_idempotent_for_unknown_id() {
    smol::block_on(async {
        let manager = manager(PermissionMode::ApproveAll);
        let response = manager
            .release_terminal(ReleaseTerminalRequest::new(
                "s1",
                TerminalId::new("does-not-exist"),
            ))
            .await
            .unwrap();
        let _ = response;
    });
}

/// Gap 20: driving a real `terminal/create` through a manager with an
/// `on_operation` callback attached observes the callback firing with the
/// expected method/status pairs (running -> completed), pure handler-level
/// (no runtime engine involved).
#[test]
fn on_operation_callback_fires_for_real_terminal_create() {
    smol::block_on(async {
        let observed: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let observed_clone = observed.clone();
        let manager = TerminalManager::new(TerminalManagerOptions {
            cwd: PathBuf::from("/tmp"),
            permission_mode: PermissionMode::ApproveAll,
            non_interactive_policy: NonInteractivePermissionPolicy::Deny,
            handler: None,
            kill_grace: Some(Duration::from_millis(300)),
        })
        .with_on_operation(Arc::new(move |operation: ClientOperation| {
            observed_clone
                .lock()
                .push((operation.method, operation.status));
        }));

        let created = manager
            .create_terminal(CreateTerminalRequest::new("s1", "echo"))
            .await
            .unwrap();
        manager
            .wait_for_terminal_exit(WaitForTerminalExitRequest::new("s1", created.terminal_id))
            .await
            .unwrap();

        let calls = observed.lock().clone();
        assert!(
            calls.contains(&("terminal/create".to_string(), "running".to_string())),
            "expected a running terminal/create operation, got {calls:?}"
        );
        assert!(
            calls.contains(&("terminal/create".to_string(), "completed".to_string())),
            "expected a completed terminal/create operation, got {calls:?}"
        );
        assert!(
            calls.contains(&(
                "terminal/wait_for_exit".to_string(),
                "completed".to_string()
            )),
            "expected a completed terminal/wait_for_exit operation, got {calls:?}"
        );
    });
}
