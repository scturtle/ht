use super::*;
use base64::Engine as _;

#[test]
fn graphics_over_budget_finish_without_another_input_or_core_event() {
    crate::kitty_graphics::set_enabled(true);
    let mut model = Model::new("budget-test".into());
    let id = model
        .insert(json!({"address":"test", "state":"live", "pid":7}))
        .unwrap();
    let mut view = View::new(120, 40, 1).unwrap();
    // Two incompressible images fit individually, but not together, within the
    // real 30 MiB frame budget. Chunk the uploads exactly as a program would.
    let mut random = 0x1234_5678u32;
    for image in 1..=2 {
        let rgba = (0..1536 * 2048 * 4)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                random as u8
            })
            .collect::<Vec<_>>();
        let payload = base64::engine::general_purpose::STANDARD.encode(rgba);
        let chunks = payload.as_bytes().chunks(4096);
        let count = chunks.len();
        for (index, chunk) in chunks.enumerate() {
            let more = u8::from(index + 1 != count);
            let header = if index == 0 {
                format!("\x1b[H\x1b_Ga=T,f=32,i={image},p=1,s=1536,v=2048,c=1,r=1,q=2,m={more};")
            } else {
                format!("\x1b_Gm={more};")
            };
            let mut packet = header.into_bytes();
            packet.extend_from_slice(chunk);
            packet.extend_from_slice(b"\x1b\\");
            view.process_output(id, &packet);
        }
    }
    let (_send, receive) = std::sync::mpsc::channel();
    let config = crate::config::Config::default();
    let mut renderer = Renderer::new().unwrap();
    renderer.set_cell_size((8, 16));
    let mut session = Session {
        core: super::super::transport::test_core(receive),
        options: None,
        model,
        shell: Vec::new(),
        views: HashMap::from([(id, view)]),
        channels: HashMap::new(),
        document_version: 1,
        size: ClientSurfaceSize {
            cols: 120,
            rows: 40,
        },
        cells: (8, 16),
        ui: view::ChromeConfig::from_config(&config),
        renderer,
        dirty: true,
        snapshot_dirty: true,
        focused: true,
        last_focus: None,
        home: None,
        _lock: None,
        events: Vec::new(),
        seq: 0,
        theme: Default::default(),
        display: json!({}),
    };
    let assets = |messages: Vec<ServerMessage>| {
        messages
            .into_iter()
            .filter_map(|m| match m {
                ServerMessage::PaneSurface(surface) => Some(surface.graphics.assets.len()),
                _ => None,
            })
            .sum::<usize>()
    };
    let first = session.publish().unwrap();
    assert!(matches!(
        first.first(),
        Some(ServerMessage::ClientShellSnapshot(_))
    ));
    assert_eq!(assets(first), 1);
    assert!(
        !super::super::should_wait_for_work(false, &session),
        "graphics continuation cannot park"
    );
    assert!(
        session.dirty,
        "undelivered assets must schedule another projection"
    );
    assert_eq!(assets(session.publish().unwrap()), 1);
    assert!(!session.dirty);
    assert!(super::super::should_wait_for_work(false, &session));
    assert!(session.publish().unwrap().is_empty());
}
