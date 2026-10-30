// Smoke: parse the real prod inbox payload shape (with forward) through
// InboxListInfo. Run: cargo run -p jcode-base --example fwd_smoke
use jcode_base::chat::InboxListInfo;

fn main() {
    let payload = r#"{
        "status": "open",
        "inbox_id": "1941464e",
        "files_count": 2,
        "files": [
            {
                "file_id": "a679985f",
                "name": "photo.jpg",
                "mime_type": "image/jpeg",
                "size": 104133,
                "uploaded_at": "2026-10-30T12:44:48.937542Z",
                "seen_at": "2026-10-30T12:45:27.388323Z",
                "forward": {
                    "forward_from": 268625244,
                    "forward_from_user_name": "agerman",
                    "forward_date": "2026-10-30T12:22:03Z"
                }
            },
            {
                "file_id": "plain",
                "name": "not-forwarded.txt",
                "mime_type": "text/plain",
                "size": 12,
                "uploaded_at": "2026-10-30T12:44:48.958387Z"
            }
        ],
        "created_at": "2026-10-30T07:03:15.236695Z",
        "expires_at": "2026-10-31T07:03:15.236695Z"
    }"#;
    let info: InboxListInfo = serde_json::from_str(payload).expect("parse inbox payload");
    assert_eq!(info.files.len(), 2);
    let fwd = info.files[0].forward.as_ref().expect("forward present");
    assert_eq!(fwd.forward_from, Some(268625244));
    assert_eq!(fwd.forward_from_user_name.as_deref(), Some("agerman"));
    assert!(info.files[1].forward.is_none(), "no forward for plain file");
    println!("OK: forward parsed, plain file has no forward");
}
