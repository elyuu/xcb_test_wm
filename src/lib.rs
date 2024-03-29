use std::cmp;
use std::collections::HashMap;
use x11;
use xcb;
use xcb_util::{ewmh, icccm, keysyms};

struct Position {
    x: i16,
    y: i16,
}

struct Size {
    width: i16,
    height: i16,
}

#[allow(non_snake_case)]
struct InternedAtoms {
    WM_PROTOCOLS: xcb::Atom,
    WM_DELETE_WINDOW: xcb::Atom,
}

#[allow(non_snake_case)]
impl InternedAtoms {
    pub fn new(connection: &xcb::Connection) -> InternedAtoms {
        let WM_PROTOCOLS = xcb::intern_atom(&connection, false, "WM_PROTOCOLS")
            .get_reply()
            .expect("Could not intern atom")
            .atom();
        let WM_DELETE_WINDOW = xcb::intern_atom(&connection, false, "WM_DELETE_WINDOW")
            .get_reply()
            .expect("Could not intern atom")
            .atom();

        InternedAtoms {
            WM_PROTOCOLS,
            WM_DELETE_WINDOW,
        }
    }
}

pub struct WindowManager {
    connection: xcb::Connection,
    root: xcb::Window,
    clients: HashMap<xcb::Window, xcb::Window>,
    drag_start: Position,
    drag_start_frame: Position,
    drag_start_frame_size: Size,
    atoms: InternedAtoms,
}

impl WindowManager {
    pub fn new() -> WindowManager {
        // Connect to default display
        let (connection, root_idx) =
            xcb::Connection::connect(None).expect("Could not connect to X display");

        // Get default root window
        let root = connection
            .get_setup()
            .roots()
            .nth(root_idx as usize)
            .expect("Could not get root window")
            .root();

        let clients = HashMap::new();

        let atoms = InternedAtoms::new(&connection);

        WindowManager {
            connection,
            root,
            clients,
            drag_start: Position { x: 0, y: 0 },
            drag_start_frame: Position { x: 0, y: 0 },
            drag_start_frame_size: Size {
                width: 0,
                height: 0,
            },
            atoms,
        }
    }

    pub fn run(&mut self) {
        // register for substructure redirect/notify
        let values = [(
            xcb::CW_EVENT_MASK,
            xcb::EVENT_MASK_SUBSTRUCTURE_NOTIFY | xcb::EVENT_MASK_SUBSTRUCTURE_REDIRECT,
        )];

        xcb::change_window_attributes_checked(&self.connection, self.root, &values)
            .request_check()
            .expect("Could not register for substructure redirect/notify");

        // frame existing windows
        xcb::grab_server(&self.connection);

        let existing_windows: Vec<_> = xcb::query_tree(&self.connection, self.root)
            .get_reply()
            .expect("Could not query existing windows")
            .children()
            .iter()
            .map(|w| *w)
            .collect();

        for window in existing_windows {
            self.frame_window(window, true);
        }

        xcb::ungrab_server(&self.connection);

        // main event loop
        loop {
            self.connection.flush();

            let e = self
                .connection
                .wait_for_event()
                .expect("Error receiving event");
            unsafe {
                match e.response_type() {
                    xcb::CONFIGURE_REQUEST => self.on_configure_request(xcb::cast_event(&e)),
                    xcb::MAP_REQUEST => self.on_map_request(xcb::cast_event(&e)),
                    xcb::UNMAP_NOTIFY => self.on_unmap_notify(xcb::cast_event(&e)),
                    xcb::BUTTON_PRESS => self.on_button_press(xcb::cast_event(&e)),
                    xcb::MOTION_NOTIFY => self.on_motion_notify(xcb::cast_event(&e)),
                    xcb::KEY_PRESS => self.on_key_press(xcb::cast_event(&e)),
                    _ => continue,
                };
            }
        }
    }

    fn on_configure_request(&self, event: &xcb::ConfigureRequestEvent) {
        // don't change anything
        let value_list = vec![
            (xcb::CONFIG_WINDOW_X as u16, event.x() as u32),
            (xcb::CONFIG_WINDOW_Y as u16, event.y() as u32),
            (xcb::CONFIG_WINDOW_WIDTH as u16, event.width() as u32),
            (xcb::CONFIG_WINDOW_HEIGHT as u16, event.height() as u32),
            (
                xcb::CONFIG_WINDOW_BORDER_WIDTH as u16,
                event.border_width() as u32,
            ),
            (xcb::CONFIG_WINDOW_SIBLING as u16, event.sibling() as u32),
            (
                xcb::CONFIG_WINDOW_STACK_MODE as u16,
                event.stack_mode() as u32,
            ),
        ];

        // if window is already managed change frame
        if self.clients.contains_key(&event.window()) {
            let frame = self
                .clients
                .get(&event.window())
                .expect("Could not retrieve window");

            xcb::configure_window(&self.connection, *frame, &value_list);
        }

        xcb::configure_window(&self.connection, self.root, &value_list);
    }

    fn on_map_request(&mut self, event: &xcb::MapRequestEvent) {
        self.frame_window(event.window(), false);
        xcb::map_window(&self.connection, event.window());
    }

    fn frame_window(&mut self, window: xcb::Window, created_before_wm: bool) {
        // panic if window is already framed
        assert!(!self.clients.contains_key(&window));

        if created_before_wm {
            let attrs = xcb::get_window_attributes(&self.connection, window)
                .get_reply()
                .expect("Could not window attributes");

            if attrs.override_redirect() || attrs.map_state() != xcb::MAP_STATE_VIEWABLE as u8 {
                return;
            }
        }

        let border_width = 4;
        let border_color = 0xff0000;
        let bg_color = 0x0000ff;

        let wid = self.connection.generate_id();
        let geo = xcb::get_geometry(&self.connection, window)
            .get_reply()
            .expect("Could not get geometry of parent window");
        let value_list = vec![
            (xcb::CW_BORDER_PIXEL as u32, border_color as u32),
            (xcb::CW_BACK_PIXEL as u32, bg_color as u32),
        ];

        // creates border window with above options
        xcb::create_window(
            &self.connection,
            xcb::COPY_FROM_PARENT as u8,
            wid,
            self.root,
            geo.x(),
            geo.y(),
            geo.width(),
            geo.height(),
            border_width,
            xcb::WINDOW_CLASS_INPUT_OUTPUT as u16,
            xcb::COPY_FROM_PARENT,
            &value_list,
        );

        // register substructure redirect/notify on new window
        let value_list = vec![(
            xcb::CW_EVENT_MASK,
            xcb::EVENT_MASK_SUBSTRUCTURE_NOTIFY | xcb::EVENT_MASK_SUBSTRUCTURE_REDIRECT,
        )];
        xcb::change_window_attributes_checked(&self.connection, wid, &value_list)
            .request_check()
            .expect("Could not register for substructure redirect/notify");

        // for cleanup?
        xcb::change_save_set(&self.connection, xcb::SET_MODE_INSERT as u8, window);

        xcb::reparent_window(&self.connection, window, wid, 0, 0);

        xcb::map_window(&self.connection, wid);

        self.clients.insert(window, wid);

        let key_symbols = keysyms::KeySymbols::new(&self.connection);

        // allows window to be moved with mod1 + left mouse button
        xcb::grab_button(
            &self.connection,
            false,
            window,
            xcb::EVENT_MASK_BUTTON_PRESS as u16
                | xcb::EVENT_MASK_BUTTON_RELEASE as u16
                | xcb::EVENT_MASK_BUTTON_MOTION as u16,
            xcb::GRAB_MODE_ASYNC as u8,
            xcb::GRAB_MODE_ASYNC as u8,
            xcb::NONE,
            xcb::NONE,
            xcb::BUTTON_INDEX_1 as u8,
            xcb::MOD_MASK_1 as u16,
        );

        // allows window to be resized with mod1 + right mouse button
        xcb::grab_button(
            &self.connection,
            false,
            window,
            xcb::EVENT_MASK_BUTTON_PRESS as u16
                | xcb::EVENT_MASK_BUTTON_RELEASE as u16
                | xcb::EVENT_MASK_BUTTON_MOTION as u16,
            xcb::GRAB_MODE_ASYNC as u8,
            xcb::GRAB_MODE_ASYNC as u8,
            xcb::NONE,
            xcb::NONE,
            xcb::BUTTON_INDEX_3 as u8,
            xcb::MOD_MASK_1 as u16,
        );

        // allows window to be closed with alt + f4
        xcb::grab_key(
            &self.connection,
            false,
            window,
            xcb::MOD_MASK_1 as u16,
            match key_symbols.get_keycode(x11::keysym::XK_F4).next() {
                Some(keycode) => keycode,
                None => panic!("Could not resolve keysym"),
            },
            xcb::GRAB_MODE_ASYNC as u8,
            xcb::GRAB_MODE_ASYNC as u8,
        );

        // allows window to be switched with alt + tab
        xcb::grab_key(
            &self.connection,
            false,
            window,
            xcb::MOD_MASK_1 as u16,
            match key_symbols.get_keycode(x11::keysym::XK_Tab).next() {
                Some(keycode) => keycode,
                None => panic!("Could not resolve keysym"),
            },
            xcb::GRAB_MODE_ASYNC as u8,
            xcb::GRAB_MODE_ASYNC as u8,
        );
    }

    fn on_unmap_notify(&mut self, event: &xcb::UnmapNotifyEvent) {
        if !self.clients.contains_key(&event.window()) {
            return;
        } else if event.event() == self.root {
            return;
        }

        self.unframe_window(event.window());
    }

    fn unframe_window(&mut self, window: xcb::Window) {
        let frame = self
            .clients
            .get(&window)
            .expect("Could not get frame window to unframe");

        xcb::unmap_window(&self.connection, *frame);

        xcb::reparent_window(&self.connection, window, self.root, 0, 0);

        xcb::change_save_set(&self.connection, xcb::SET_MODE_DELETE as u8, window);

        xcb::destroy_window(&self.connection, *frame);

        self.clients.remove(&window);
    }

    fn on_button_press(&mut self, event: &xcb::ButtonPressEvent) {
        assert!(self.clients.contains_key(&event.event()));

        let frame: xcb::Window = self.clients[&event.event()];

        self.drag_start = Position {
            x: event.root_x(),
            y: event.root_y(),
        };

        let geo = xcb::get_geometry(&self.connection, frame)
            .get_reply()
            .expect("Could not get geometry of parent window");

        self.drag_start_frame = Position {
            x: geo.x(),
            y: geo.y(),
        };

        self.drag_start_frame_size = Size {
            width: geo.width() as i16,
            height: geo.height() as i16,
        }
    }

    fn on_motion_notify(&mut self, event: &xcb::MotionNotifyEvent) {
        assert!(self.clients.contains_key(&event.event()));

        let frame: xcb::Window = self.clients[&event.event()];

        let drag_pos = Position {
            x: event.root_x(),
            y: event.root_y(),
        };

        let delta = Position {
            x: drag_pos.x - self.drag_start.x,
            y: drag_pos.y - self.drag_start.y,
        };

        if event.state() as u32 & xcb::BUTTON_MASK_1 > 0 {
            let dest_fram_pos = Position {
                x: self.drag_start_frame.x + delta.x,
                y: self.drag_start_frame.y + delta.y,
            };

            let value_list = vec![
                (xcb::CONFIG_WINDOW_X as u16, dest_fram_pos.x as u32),
                (xcb::CONFIG_WINDOW_Y as u16, dest_fram_pos.y as u32),
            ];

            xcb::configure_window(&self.connection, frame, &value_list);
        } else if event.state() as u32 & xcb::BUTTON_MASK_3 > 0 {
            let size_delta = Size {
                width: cmp::max(delta.x, -self.drag_start_frame_size.height),
                height: cmp::max(delta.y, -self.drag_start_frame_size.width),
            };

            let dest_size = Size {
                width: self.drag_start_frame_size.width + size_delta.width,
                height: self.drag_start_frame_size.height + size_delta.height,
            };

            // resize frame
            let value_list = vec![
                (xcb::CONFIG_WINDOW_WIDTH as u16, dest_size.width as u32),
                (xcb::CONFIG_WINDOW_HEIGHT as u16, dest_size.height as u32),
            ];
            xcb::configure_window(&self.connection, frame, &value_list);

            // resize client
            xcb::configure_window(&self.connection, event.event(), &value_list);
        }
    }

    fn on_key_press(&mut self, event: &xcb::KeyPressEvent) {
        let key_symbols = keysyms::KeySymbols::new(&self.connection);
        let keycode = key_symbols
            .get_keycode(x11::keysym::XK_F4)
            .next()
            .expect("Could not get keycode from key press event");

        if (event.state() as u32 & xcb::MOD_MASK_1) > 0 && (event.detail() == keycode) {
            let supported_protocols: Vec<u32> =
                icccm::get_wm_protocols(&self.connection, event.event(), self.atoms.WM_PROTOCOLS)
                    .get_reply()
                    .expect("Could not query for wm_protocols")
                    .atoms()
                    .to_vec();

            if supported_protocols.contains(&self.atoms.WM_DELETE_WINDOW) {
                let data = xcb::ClientMessageData::from_data32([
                    self.atoms.WM_DELETE_WINDOW,
                    xcb::CURRENT_TIME,
                    0,
                    0,
                    0,
                ]);

                let delete_event =
                    xcb::ClientMessageEvent::new(32, event.event(), self.atoms.WM_PROTOCOLS, data);
                xcb::send_event(
                    &self.connection,
                    false,
                    event.event(),
                    xcb::EVENT_MASK_NO_EVENT,
                    &delete_event,
                );
            } else {
                xcb::destroy_window(&self.connection, event.event());
            }
        }
    }
}
