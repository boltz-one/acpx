use super::*;

fn tempdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("boltz-acpx-fs-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn read_within_root_succeeds() {
    smol::block_on(async {
        let root = tempdir();
        std::fs::write(root.join("hello.txt"), "line1\nline2\nline3").unwrap();
        let handlers = FilesystemHandlers::new(
            &root,
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            None,
        )
        .unwrap();

        let response = handlers
            .read_text_file(ReadTextFileRequest::new("s1", root.join("hello.txt")))
            .await
            .unwrap();
        assert_eq!(response.content, "line1\nline2\nline3");
    });
}

#[test]
fn read_windowed_by_line_and_limit() {
    smol::block_on(async {
        let root = tempdir();
        std::fs::write(root.join("hello.txt"), "a\nb\nc\nd").unwrap();
        let handlers = FilesystemHandlers::new(
            &root,
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            None,
        )
        .unwrap();

        let response = handlers
            .read_text_file(
                ReadTextFileRequest::new("s1", root.join("hello.txt"))
                    .line(2u32)
                    .limit(2u32),
            )
            .await
            .unwrap();
        assert_eq!(response.content, "b\nc");
    });
}

#[test]
fn deny_all_mode_rejects_read() {
    smol::block_on(async {
        let root = tempdir();
        std::fs::write(root.join("hello.txt"), "hi").unwrap();
        let handlers = FilesystemHandlers::new(
            &root,
            PermissionMode::DenyAll,
            NonInteractivePermissionPolicy::Deny,
            None,
        )
        .unwrap();

        let result = handlers
            .read_text_file(ReadTextFileRequest::new("s1", root.join("hello.txt")))
            .await;
        assert!(matches!(result, Err(AcpError::PermissionDenied(_))));
    });
}

#[test]
fn traversal_outside_root_is_rejected() {
    smol::block_on(async {
        let root = tempdir();
        let outside = std::env::temp_dir().join("boltz-acpx-fs-outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let handlers = FilesystemHandlers::new(
            &root,
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            None,
        )
        .unwrap();

        let traversal_path = root.join("../boltz-acpx-fs-outside.txt");
        let result = handlers
            .read_text_file(ReadTextFileRequest::new("s1", traversal_path))
            .await;
        assert!(matches!(result, Err(AcpError::PermissionDenied(_))));
    });
}

#[test]
#[cfg(unix)]
fn symlink_escaping_root_is_rejected_on_write() {
    smol::block_on(async {
        let root = tempdir();
        let outside_dir = tempdir();
        std::os::unix::fs::symlink(&outside_dir, root.join("escape")).unwrap();
        let handlers = FilesystemHandlers::new(
            &root,
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            None,
        )
        .unwrap();

        let result = handlers
            .write_text_file(WriteTextFileRequest::new(
                "s1",
                root.join("escape").join("new-file.txt"),
                "content",
            ))
            .await;
        assert!(matches!(result, Err(AcpError::PermissionDenied(_))));
    });
}

#[test]
fn write_approved_creates_parent_dirs() {
    smol::block_on(async {
        let root = tempdir();
        let handlers = FilesystemHandlers::new(
            &root,
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            None,
        )
        .unwrap();

        handlers
            .write_text_file(WriteTextFileRequest::new(
                "s1",
                root.join("nested/dir/new-file.txt"),
                "hello",
            ))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("nested/dir/new-file.txt")).unwrap(),
            "hello"
        );
    });
}

#[test]
fn write_denied_in_deny_all_mode() {
    smol::block_on(async {
        let root = tempdir();
        let handlers = FilesystemHandlers::new(
            &root,
            PermissionMode::DenyAll,
            NonInteractivePermissionPolicy::Deny,
            None,
        )
        .unwrap();

        let result = handlers
            .write_text_file(WriteTextFileRequest::new(
                "s1",
                root.join("new-file.txt"),
                "hi",
            ))
            .await;
        assert!(matches!(result, Err(AcpError::PermissionDenied(_))));
    });
}

/// Gap 20: driving a real `fs/read_text_file` through a handler with an
/// `on_operation` callback attached observes the callback firing with the
/// expected method/status pairs (running -> completed), pure handler-level
/// (no runtime engine involved).
#[test]
fn on_operation_callback_fires_for_real_read_text_file() {
    smol::block_on(async {
        let root = tempdir();
        std::fs::write(root.join("hello.txt"), "hi").unwrap();
        let observed: std::sync::Arc<parking_lot::Mutex<Vec<(String, String)>>> =
            std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let observed_clone = observed.clone();
        let handlers = FilesystemHandlers::new(
            &root,
            PermissionMode::ApproveAll,
            NonInteractivePermissionPolicy::Deny,
            None,
        )
        .unwrap()
        .with_on_operation(std::sync::Arc::new(move |operation: ClientOperation| {
            observed_clone
                .lock()
                .push((operation.method, operation.status));
        }));

        handlers
            .read_text_file(ReadTextFileRequest::new("s1", root.join("hello.txt")))
            .await
            .unwrap();

        let calls = observed.lock().clone();
        assert!(
            calls.contains(&("fs/read_text_file".to_string(), "running".to_string())),
            "expected a running fs/read_text_file operation, got {calls:?}"
        );
        assert!(
            calls.contains(&("fs/read_text_file".to_string(), "completed".to_string())),
            "expected a completed fs/read_text_file operation, got {calls:?}"
        );
    });
}
