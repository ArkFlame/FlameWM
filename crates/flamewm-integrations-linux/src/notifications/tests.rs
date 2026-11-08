use super::*;

fn hints(items: Vec<(String, MessageItem)>) -> dbus::arg::messageitem::MessageItemDict {
    let dict = MessageItem::from_dict(items.into_iter().map(Ok::<_, ()>)).unwrap();
    let MessageItem::Dict(dict) = dict else {
        panic!("dictionary")
    };
    dict
}

#[test]
fn urgency_progress_and_timeout_hints_are_bounded() {
    let dict = hints(vec![
        ("urgency".to_owned(), MessageItem::Byte(2)),
        ("value".to_owned(), MessageItem::Int32(55)),
        ("transient".to_owned(), MessageItem::Bool(true)),
        ("resident".to_owned(), MessageItem::Bool(true)),
    ]);
    assert_eq!(
        parse_hints(&dict),
        Hints {
            urgency: NotificationUrgency::Critical,
            progress: Some(55),
            transient: true,
            resident: true,
            image: None,
            image_path: None,
            desktop_entry: None,
        }
    );
    let out_of_range = hints(vec![("value".to_owned(), MessageItem::Int32(101))]);
    assert_eq!(parse_hints(&out_of_range).progress, None);
    assert_eq!(
        expiration(i32::MAX, NotificationUrgency::Normal),
        Some(MAX_TIMEOUT)
    );
    assert_eq!(expiration(0, NotificationUrgency::Normal), None);
    assert_eq!(
        expiration(-1, NotificationUrgency::Normal),
        Some(DEFAULT_TIMEOUT)
    );
    assert_eq!(
        expiration(500, NotificationUrgency::Normal),
        Some(Duration::from_millis(500))
    );
}

#[test]
fn image_data_is_downscaled_once_to_icon_edge() {
    // 96x2 RGB: step 2 gives 48x1, sampling every other column.
    let mut data = Vec::new();
    for x in 0..96u32 {
        data.extend_from_slice(&[x as u8, 0, 0]);
    }
    let image = downscale_to_icon(96, 1, 3, 96 * 3, &data).expect("valid image");
    assert_eq!((image.width, image.height), (48, 1));
    assert_eq!(image.rgba.len(), 48 * 4);
    assert_eq!(&image.rgba[..4], &[0, 0, 0, 255]);
    assert_eq!(&image.rgba[4..8], &[2, 0, 0, 255]);

    // Small RGBA images are kept at native size and keep their alpha.
    let small = downscale_to_icon(1, 1, 4, 4, &[9, 8, 7, 6]).expect("valid image");
    assert_eq!(small.rgba, [9, 8, 7, 6]);
    assert!(downscale_to_icon(0, 1, 4, 4, &[]).is_none());
    assert!(downscale_to_icon(1, 1, 2, 2, &[0, 0]).is_none());
}

#[test]
fn image_data_hint_rejects_oversized_and_mismatched_payloads() {
    let image = |width: i32, height: i32, stride: i32, bytes: usize| {
        MessageItem::Struct(vec![
            MessageItem::Int32(width),
            MessageItem::Int32(height),
            MessageItem::Int32(stride),
            MessageItem::Bool(true),
            MessageItem::Int32(8),
            MessageItem::Int32(4),
            MessageItem::new_array((0..bytes).map(|_| MessageItem::Byte(0)).collect())
                .expect("bytes"),
        ])
    };
    let accepted = image_data_hint(&image(2, 2, 8, 16)).expect("2x2 RGBA");
    assert_eq!(
        (accepted.width, accepted.height, accepted.rgba.len()),
        (2, 2, 16)
    );
    assert!(image_data_hint(&image(513, 1, 2052, 2052)).is_none());
    assert!(image_data_hint(&image(2, 2, 8, 15)).is_none());
    assert!(image_data_hint(&MessageItem::Str("x".to_owned())).is_none());
}

#[test]
fn icon_name_precedence_is_image_path_then_app_icon_then_desktop_entry() {
    let base = Hints {
        urgency: NotificationUrgency::Normal,
        progress: None,
        transient: false,
        resident: false,
        image: None,
        image_path: None,
        desktop_entry: Some("org.example.App".to_owned()),
    };
    assert_eq!(icon_name_for(&base, "mail").as_deref(), Some("mail"));
    let with_path = Hints {
        image_path: Some("camera-photo".to_owned()),
        ..base.clone()
    };
    assert_eq!(
        icon_name_for(&with_path, "mail").as_deref(),
        Some("camera-photo")
    );
    assert_eq!(icon_name_for(&base, "").as_deref(), Some("org.example.App"));
    let absolute = Hints {
        image_path: Some("/tmp/x.png".to_owned()),
        ..base.clone()
    };
    assert_eq!(icon_name_for(&absolute, "mail").as_deref(), Some("mail"));
}

/// Stored-deflate RGBA PNG whose pixels are all opaque grey, for `image-path` tests.
fn png_fixture(width: u32, height: u32) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::new();
    for _ in 0..height {
        raw.push(0);
        raw.extend(std::iter::repeat_n(0x80u8, width as usize * 4));
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::none());
    std::io::Write::write_all(&mut encoder, &raw).unwrap();
    let idat = encoder.finish().unwrap();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, *b"IHDR", &ihdr);
    chunk(&mut out, *b"IDAT", &idat);
    chunk(&mut out, *b"IEND", &[]);
    out
}

#[test]
fn image_path_decodes_a_bounded_png_downscaled_once() {
    let dir = std::env::temp_dir().join(format!("flamewm-nf-image-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wide = dir.join("wide.png");
    std::fs::write(&wide, png_fixture(96, 2)).unwrap();
    let image = image_path_hint(wide.to_str().unwrap()).expect("96x2 png decodes");
    assert_eq!(
        (image.width, image.height),
        (48, 1),
        "one step of 2 per axis"
    );
    assert_eq!(image.rgba.len(), 48 * 4);
    assert!(image_path_hint(&format!("file://{}", wide.display())).is_some());
    assert!(
        image_path_hint("camera-photo").is_none(),
        "names use the icon theme"
    );
    assert!(image_path_hint("relative/x.png").is_none());
    assert!(
        image_path_hint("/dev/null").is_none(),
        "only regular files are read"
    );
    assert!(image_path_hint(dir.join("missing.png").to_str().unwrap()).is_none());
    let text = dir.join("text.png");
    std::fs::write(&text, b"plain text that is not a png file at all").unwrap();
    assert!(image_path_hint(text.to_str().unwrap()).is_none());
    let too_wide = dir.join("too-wide.png");
    std::fs::write(&too_wide, png_fixture(513, 1)).unwrap();
    assert!(image_path_hint(too_wide.to_str().unwrap()).is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn percent_decode_handles_file_uri_escapes() {
    assert_eq!(
        percent_decode("/a%20b/c.png").as_deref(),
        Some("/a b/c.png")
    );
    assert_eq!(percent_decode("/a%2"), None);
    assert_eq!(percent_decode("/a%zz"), None);
}

#[test]
fn expire_timeout_follows_urgency_policy() {
    // Critical never auto-expires, even with an explicit timeout.
    assert_eq!(expiration(-1, NotificationUrgency::Critical), None);
    assert_eq!(expiration(4000, NotificationUrgency::Critical), None);
    // Low urgency defaults shorter than normal; an explicit timeout is honored.
    assert_eq!(
        expiration(-1, NotificationUrgency::Low),
        Some(LOW_URGENCY_TIMEOUT)
    );
    assert_eq!(
        expiration(9000, NotificationUrgency::Low),
        Some(Duration::from_secs(9))
    );
}

#[test]
fn close_reason_codes_match_the_spec() {
    assert_eq!(NotificationCloseReason::Expired.wire_value(), 1);
    assert_eq!(NotificationCloseReason::Dismissed.wire_value(), 2);
    assert_eq!(NotificationCloseReason::Closed.wire_value(), 3);
    assert_eq!(NotificationCloseReason::Undefined.wire_value(), 4);
}

#[test]
fn actions_parse_pairs_with_default_and_visible_cap() {
    let items = [
        MessageItem::Str("default".to_owned()),
        MessageItem::Str("Open".to_owned()),
        MessageItem::Str("reply".to_owned()),
        MessageItem::Str("Reply".to_owned()),
        MessageItem::Str("a".to_owned()),
        MessageItem::Str("A".to_owned()),
        MessageItem::Str("b".to_owned()),
        MessageItem::Str("B".to_owned()),
        MessageItem::Str("c".to_owned()),
        MessageItem::Str("C".to_owned()),
        MessageItem::Str("dangling".to_owned()),
    ];
    let (actions, default_action) = parse_actions(&items);
    assert!(default_action);
    let keys: Vec<_> = actions.iter().map(|action| action.key.as_str()).collect();
    assert_eq!(keys, ["reply", "a", "b"]);
    assert_eq!(actions[0].label, "Reply");

    let view = NotificationView {
        id: 1,
        app_name: String::new(),
        summary: String::new(),
        body: String::new(),
        urgency: NotificationUrgency::Normal,
        progress: None,
        actions,
        default_action,
        image: None,
        icon_name: None,
    };
    assert!(action_advertised(&view, "default"));
    assert!(action_advertised(&view, "b"));
    assert!(
        !action_advertised(&view, "c"),
        "hidden fourth action is not invokable"
    );
    assert!(!action_advertised(&view, "missing"));
}

#[test]
fn markup_is_reduced_to_safe_plain_text() {
    assert_eq!(
        sanitize_markup("<b>bold</b> <i>it</i> <u>u</u>"),
        "bold it u"
    );
    assert_eq!(
        sanitize_markup(r#"<a href="javascript:alert(1)">link</a>"#),
        "link"
    );
    assert_eq!(sanitize_markup("<img src=x onerror=y>x"), "x");
    assert_eq!(
        sanitize_markup("a &amp; b &lt;c&gt; &quot;q&quot; &#65;&#x42;"),
        "a & b <c> \"q\" AB"
    );
    // Entities decode to text only; decoded markup is never re-parsed.
    assert_eq!(sanitize_markup("&lt;script&gt;"), "<script>");
    assert_eq!(sanitize_markup("1 < 2 and 3 > 2"), "1 < 2 and 3 > 2");
    assert_eq!(sanitize_markup("line<br>two"), "line\ntwo");
    assert_eq!(sanitize_markup("unterminated <b"), "unterminated <b");
    // Control characters and unknown names stay literal instead of decoding.
    assert_eq!(sanitize_markup("&#0;&unknown;"), "&#0;&unknown;");
}

#[test]
fn image_hint_caps_are_checked_before_decode() {
    // 2x2 RGBA with a matching rowstride and byte count is accepted.
    assert!(image_data_within_caps(2, 2, 8, 8, 4, 16));
    assert!(!image_data_within_caps(513, 1, 2052, 8, 4, 2052));
    assert!(
        !image_data_within_caps(2, 2, 8, 16, 4, 16),
        "only 8-bit samples"
    );
    assert!(
        !image_data_within_caps(2, 2, 8, 8, 2, 16),
        "only RGB or RGBA"
    );
    assert!(
        !image_data_within_caps(2, 2, 8, 8, 4, 15),
        "byte count mismatch"
    );
    assert!(
        !image_data_within_caps(2, 2, 7, 8, 4, 14),
        "rowstride too small"
    );
    assert!(!image_data_within_caps(
        512,
        512,
        2048,
        8,
        4,
        512 * 2048 + 1
    ));
    assert!(image_data_within_caps(512, 512, 2048, 8, 4, 512 * 2048));
    assert!(!image_data_within_caps(0, 2, 8, 8, 4, 0));
}

#[test]
fn id_allocation_wraps_without_reusing_live_ids() {
    let mut active = BTreeMap::new();
    active.insert(
        1,
        Notification {
            view: NotificationView {
                id: 1,
                app_name: String::new(),
                summary: String::new(),
                body: String::new(),
                urgency: NotificationUrgency::Normal,
                progress: None,
                actions: Vec::new(),
                default_action: false,
                image: None,
                icon_name: None,
            },
            expires_at: None,
            resident: false,
        },
    );
    let mut next = u32::MAX;
    assert_eq!(allocate_id(&active, &mut next), Some(u32::MAX));
    assert_eq!(allocate_id(&active, &mut next), Some(2));
}

#[test]
fn ui_queue_overflow_resets_to_authoritative_active_state() {
    let active = BTreeMap::from([(
        7,
        Notification {
            view: NotificationView {
                id: 7,
                app_name: "Files".to_owned(),
                summary: "Copying".to_owned(),
                body: "one file".to_owned(),
                urgency: NotificationUrgency::Normal,
                progress: Some(50),
                actions: Vec::new(),
                default_action: false,
                image: None,
                icon_name: None,
            },
            expires_at: None,
            resident: false,
        },
    )]);
    let mut updates = VecDeque::from(vec![
        NotificationUiUpdate::Close {
            id: 1,
            reason: NotificationCloseReason::Closed,
        };
        MAX_UI_UPDATES
    ]);
    push_update(
        &mut updates,
        &active,
        NotificationUiUpdate::Close {
            id: 2,
            reason: NotificationCloseReason::Closed,
        },
    );
    assert_eq!(updates.len(), 1);
    assert_eq!(
        updates.front(),
        Some(&NotificationUiUpdate::Reset(vec![active[&7].view.clone()]))
    );
}

#[test]
fn close_signal_carries_id_and_standard_reason() {
    let signal = closed_signal(42, NotificationCloseReason::Expired);
    assert_eq!(signal.member().unwrap().to_string(), "NotificationClosed");
    assert_eq!(
        signal.get_items(),
        vec![MessageItem::UInt32(42), MessageItem::UInt32(1)]
    );
}

fn notify_call(app: &str, summary: &str, urgency: u8) -> Message {
    let mut call = Message::new_method_call(
        NOTIFICATIONS_SERVICE,
        NOTIFICATIONS_PATH,
        NOTIFICATIONS_INTERFACE,
        "Notify",
    )
    .expect("notify call");
    let hints = MessageItem::from_dict(
        vec![Ok::<_, ()>((
            "urgency".to_owned(),
            MessageItem::Byte(urgency),
        ))]
        .into_iter(),
    )
    .expect("hints");
    // Replies need a non-zero serial, which a real bus assigns on send.
    call.set_serial(1);
    call.append_items(&[
        MessageItem::Str(app.to_owned()),
        MessageItem::UInt32(0),
        MessageItem::Str(String::new()),
        MessageItem::Str(summary.to_owned()),
        MessageItem::Str(String::new()),
        MessageItem::new_array(vec![MessageItem::Str("default".to_owned())]).expect("actions"),
        hints,
        MessageItem::Int32(-1),
    ]);
    call
}

struct Inbox {
    notifications: BTreeMap<u32, Notification>,
    updates: VecDeque<NotificationUiUpdate>,
    next_id: u32,
    history: NotificationHistory,
}

impl Inbox {
    fn new(history: NotificationHistory) -> Self {
        Self {
            notifications: BTreeMap::new(),
            updates: VecDeque::new(),
            next_id: 1,
            history,
        }
    }

    fn notify(&mut self, app: &str, summary: &str, urgency: u8) -> MessageDisposition {
        handle_message(
            &notify_call(app, summary, urgency),
            true,
            &mut self.notifications,
            &mut self.updates,
            &mut self.next_id,
            &mut self.history,
            &mut Vec::new(),
            Instant::now(),
        )
    }
}

#[test]
fn dnd_records_but_does_not_toast_until_critical() {
    let mut inbox = Inbox::new(NotificationHistory::new(10));
    inbox.history.set_dnd(true);

    assert!(matches!(
        inbox.notify("chat", "quiet", 1),
        MessageDisposition::Reply(_)
    ));
    assert!(inbox.notifications.is_empty());
    assert!(inbox.updates.is_empty());
    assert_eq!(inbox.history.len(), 1);

    inbox.notify("chat", "alarm", 2);
    assert_eq!(inbox.notifications.len(), 1);
    assert_eq!(inbox.updates.len(), 1);
    assert_eq!(inbox.history.len(), 2);
}

#[test]
fn muted_app_is_recorded_without_toast_even_when_critical() {
    let mut history = NotificationHistory::new(10);
    history.set_app_muted("spam", true);
    let mut inbox = Inbox::new(history);

    inbox.notify("spam", "urgent", 2);
    assert!(inbox.notifications.is_empty());
    assert_eq!(inbox.history.len(), 1);
    assert!(inbox.history.groups()[0].muted);
}

#[test]
fn normal_notification_toasts_and_is_recorded_once() {
    let mut inbox = Inbox::new(NotificationHistory::new(10));
    inbox.notify("mail", "hello", 1);
    assert_eq!(inbox.notifications.len(), 1);
    assert_eq!(inbox.history.len(), 1);
    assert_eq!(
        inbox.history.entries().next().unwrap().view.summary,
        "hello"
    );
}

#[test]
fn transient_skips_history_and_markup_is_stripped_on_notify() {
    let mut inbox = Inbox::new(NotificationHistory::new(10));
    let transient = MessageItem::from_dict(
        vec![
            Ok::<_, ()>(("transient".to_owned(), MessageItem::Bool(true))),
            Ok(("resident".to_owned(), MessageItem::Bool(true))),
        ]
        .into_iter(),
    )
    .expect("hints");
    let mut call = Message::new_method_call(
        NOTIFICATIONS_SERVICE,
        NOTIFICATIONS_PATH,
        NOTIFICATIONS_INTERFACE,
        "Notify",
    )
    .expect("notify call");
    call.set_serial(1);
    call.append_items(&[
        MessageItem::Str("chat".to_owned()),
        MessageItem::UInt32(0),
        MessageItem::Str(String::new()),
        MessageItem::Str("<b>hi</b>".to_owned()),
        MessageItem::Str(String::new()),
        MessageItem::new_array(vec![
            MessageItem::Str("default".to_owned()),
            MessageItem::Str("Open".to_owned()),
        ])
        .expect("actions"),
        transient,
        MessageItem::Int32(-1),
    ]);
    assert!(matches!(
        handle_message(
            &call,
            true,
            &mut inbox.notifications,
            &mut inbox.updates,
            &mut inbox.next_id,
            &mut inbox.history,
            &mut Vec::new(),
            Instant::now(),
        ),
        MessageDisposition::Reply(_)
    ));
    assert_eq!(inbox.history.len(), 0, "transient is not recorded");
    let view = &inbox.notifications[&1].view;
    assert_eq!(view.summary, "hi");
    assert!(inbox.notifications[&1].resident);
}
