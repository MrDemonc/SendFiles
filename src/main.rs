use gtk4::prelude::*;
use libadwaita::{Application, ApplicationWindow, HeaderBar, Avatar};
use gtk4::{Button, Align, Box, Orientation, ProgressBar, FlowBox};
use gtk4::glib;
use std::sync::{Arc, Mutex};
use std::cell::RefCell;
use std::path::PathBuf;
use tokio::runtime::Runtime;

const APP_ID: &str = "com.example.SendFiles";

// Custom events to communicate from background service to UI
#[derive(Clone)]
enum ServiceEvent {
    EndpointFound(rqs_lib::EndpointInfo),
    InboundRequest {
        id: String,
        device_name: String,
        files_count: usize,
        total_bytes: u64,
    },
    OutboundStarted(String),
    Progress(f64),
    Finished,
    Error(String),
}

// Global application state to store channels and runtime
struct AppState {
    runtime: Runtime,
    file_sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<rqs_lib::SendInfo>>>>,
    msg_sender: Arc<Mutex<Option<tokio::sync::broadcast::Sender<rqs_lib::channel::ChannelMessage>>>>,
    event_receiver: async_channel::Receiver<ServiceEvent>,
}

// We use a thread-local to access the state from GTK callbacks (which run on the main thread)
thread_local! {
    static APP_STATE: RefCell<Option<Arc<AppState>>> = RefCell::new(None);
}

fn main() {
    // Initialize libadwaita
    libadwaita::init().expect("Failed to initialize Libadwaita.");

    // Initialize Tokio Runtime
    let runtime = Runtime::new().expect("Failed to create Tokio runtime");

    // Channels for UI <-> Background communication
    let (event_tx, event_rx) = async_channel::unbounded::<ServiceEvent>();
    let file_sender_container = Arc::new(Mutex::new(None));
    let msg_sender_container = Arc::new(Mutex::new(None));

    // Prepare state to store in thread_local
    let app_state = Arc::new(AppState {
        runtime,
        file_sender: file_sender_container.clone(),
        msg_sender: msg_sender_container.clone(),
        event_receiver: event_rx,
    });

    APP_STATE.with(|state| {
        *state.borrow_mut() = Some(app_state.clone());
    });

    // Start Central RQS Service in background
    let etx = event_tx.clone();
    app_state.runtime.spawn(async move {
        println!("Starting Central RQS Service...");
        
        let downloads_path = glib::user_special_dir(glib::UserDirectory::Downloads)
            .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Downloads"));

        let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .or_else(|_| std::fs::read_to_string("/etc/hostname"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "Linux".to_string());
        println!("Initializing RQS with name: {}", hostname);

        let mut rqs = rqs_lib::RQS::new(
            rqs_lib::Visibility::Visible,
            None,
            Some(downloads_path),
            Some(hostname),
        );

        let run_result = rqs.run().await;
        let msg_sender = rqs.message_sender.clone();
        let mut msg_rx = msg_sender.subscribe();

        // Store channels for UI to use
        *msg_sender_container.lock().unwrap() = Some(msg_sender);

        if let Ok((fs, _ble_receiver)) = run_result {
            *file_sender_container.lock().unwrap() = Some(fs);
            println!("RQS Service Started Successfully");

            // Start mDNS Discovery
            let (mdns_tx, mdns_rx) = tokio::sync::broadcast::channel(100);
            if let Err(e) = rqs.discovery(mdns_tx) {
                eprintln!("Error starting discovery: {:?}", e);
            } else {
                println!("Discovery started successfully");
            }
            
            let etx_ep = etx.clone();
            let mdns_rx_loop = mdns_rx;
            let rt_handle = tokio::runtime::Handle::current();
            rt_handle.spawn(async move {
                let mut rx = mdns_rx_loop;
                loop {
                    match rx.recv().await {
                        Ok(endpoint) => {
                            let _ = etx_ep.send(ServiceEvent::EndpointFound(endpoint)).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("Discovery loop lagged by {} messages", n);
                            continue;
                        }
                        Err(_) => {
                            eprintln!("Discovery loop connection lost");
                            break;
                        }
                    }
                }
            });

            // Background Event Listener Loop
            loop {
                match msg_rx.recv().await {
                    Ok(channel_msg) => {
                        let id = channel_msg.id.clone();
                        let msg = channel_msg.msg;
                        
                        // Filter for client messages (events from lib)
                        if let Some(client_msg) = msg.as_client() {
                            use rqs_lib::TransferState;
                            let state = client_msg.state.clone().unwrap_or(TransferState::Initial);
                            println!("DEBUG: Background event - ID: {}, State: {:?}", id, state);
                            
                            match state {
                                TransferState::WaitingForUserConsent => {
                                    let device_name = client_msg.metadata.as_ref()
                                        .and_then(|m| m.source.as_ref())
                                        .map(|s| s.name.clone())
                                        .unwrap_or_else(|| "Unknown Device".to_string());
                                    
                                    let files_count = client_msg.metadata.as_ref()
                                        .and_then(|m| m.payload.as_ref())
                                        .map(|p| match p {
                                            rqs_lib::hdl::info::TransferPayload::Files(f) => f.len(),
                                            _ => 0,
                                        })
                                        .unwrap_or(0);
                                    
                                    let total_bytes = client_msg.metadata.as_ref().map(|m| m.total_bytes).unwrap_or(0);
                                    
                                    let _ = etx.send(ServiceEvent::InboundRequest {
                                        id,
                                        device_name,
                                        files_count,
                                        total_bytes,
                                    }).await;
                                }
                                TransferState::ReceivingFiles | TransferState::SendingFiles => {
                                    if let Some(meta) = &client_msg.metadata {
                                        // If it's the start of an outbound transfer, notify UI of the ID
                                        if let TransferState::SendingFiles = client_msg.state.clone().unwrap_or(TransferState::Initial) {
                                            let _ = etx.send(ServiceEvent::OutboundStarted(id)).await;
                                        }

                                        if meta.total_bytes > 0 {
                                            let progress = meta.ack_bytes as f64 / meta.total_bytes as f64;
                                            let _ = etx.send(ServiceEvent::Progress(progress)).await;
                                        }
                                    }
                                }
                                TransferState::Finished => {
                                    let _ = etx.send(ServiceEvent::Finished).await;
                                }
                                TransferState::Rejected | TransferState::Cancelled | TransferState::Disconnected => {
                                    let _ = etx.send(ServiceEvent::Error("Transfer ended prematurely".to_string())).await;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("Warning: Event loop lagged by {} messages", n);
                        continue;
                    }
                    Err(_) => {
                        eprintln!("Critical: Background event loop connection lost");
                        break;
                    }
                }
            }
        } else {
            eprintln!("Failed to start RQS Service");
            let _ = etx.send(ServiceEvent::Error("Failed to start service".to_string())).await;
        }
    });

    let app = Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(build_ui);

    app.run();
}

fn build_ui(app: &Application) {
    let content = Box::new(Orientation::Vertical, 0);
    let header_bar = HeaderBar::new();
    content.append(&header_bar);

    let stack = gtk4::Stack::new();
    stack.set_transition_type(gtk4::StackTransitionType::SlideLeftRight);
    
    // --- Page 1: Initial State ---
    let page1_box = Box::new(Orientation::Vertical, 0);
    page1_box.set_valign(Align::Center);
    page1_box.set_halign(Align::Center);

    let send_button = Button::builder()
        .label("Send File")
        .build();
    send_button.set_height_request(60);
    send_button.set_width_request(160);
    send_button.add_css_class("suggested-action");
    send_button.add_css_class("pill");

    page1_box.append(&send_button);
    stack.add_named(&page1_box, Some("initial"));

    // --- Page 2: Selected State ---
    let page2_box = Box::new(Orientation::Vertical, 12);
    page2_box.set_valign(Align::Center);
    page2_box.set_halign(Align::Center);
    page2_box.set_margin_top(24);
    page2_box.set_margin_bottom(24);
    page2_box.set_margin_start(24);
    page2_box.set_margin_end(24);

    // File Info
    let file_icon = gtk4::Image::from_icon_name("text-x-generic-symbolic");
    file_icon.set_pixel_size(64);
    let file_name_label = gtk4::Label::new(Some("No file selected"));
    file_name_label.add_css_class("title-3");
    
    // Devices Section
    let devices_label = gtk4::Label::new(Some("Select a device to share with:"));
    
    // Device List (FlowBox)
    let devices_flowbox = FlowBox::new();
    devices_flowbox.set_homogeneous(true);
    devices_flowbox.set_selection_mode(gtk4::SelectionMode::None);
    devices_flowbox.set_min_children_per_line(3);
    devices_flowbox.set_max_children_per_line(5);
    devices_flowbox.set_valign(Align::Start);
    devices_flowbox.set_height_request(150);

    let devices_scroller = gtk4::ScrolledWindow::new();
    devices_scroller.set_child(Some(&devices_flowbox));
    devices_scroller.set_height_request(160);
    devices_scroller.set_min_content_height(160);
    // Add a frame or visual distinction if needed, for now just the scroller

    // Progress Bar
    let progress_bar = ProgressBar::new();
    progress_bar.set_visible(false);
    progress_bar.set_text(Some("Sending..."));
    progress_bar.set_show_text(true);


    // Cancel Button
    let cancel_button = Button::with_label("Cancel");
    cancel_button.add_css_class("destructive-action");

    page2_box.append(&file_icon);
    page2_box.append(&file_name_label);
    page2_box.append(&devices_label);
    page2_box.append(&devices_scroller);
    page2_box.append(&progress_bar);
    page2_box.append(&cancel_button);

    stack.add_named(&page2_box, Some("selected"));

    // --- Page 3: Request State ---
    let page3_box = Box::new(Orientation::Vertical, 12);
    page3_box.set_valign(Align::Center);
    page3_box.set_halign(Align::Center);
    page3_box.set_margin_bottom(24);
    page3_box.set_margin_top(24);
    page3_box.set_margin_start(24);
    page3_box.set_margin_end(24);

    let req_icon = gtk4::Image::from_icon_name("document-import-symbolic");
    req_icon.set_pixel_size(64);
    
    let req_title = gtk4::Label::new(Some("Incoming Transfer"));
    req_title.add_css_class("title-2");

    let req_device_label = gtk4::Label::new(None);
    req_device_label.add_css_class("title-4");

    let req_files_label = gtk4::Label::new(None);
    req_files_label.add_css_class("dimmed");

    let req_progress_bar = ProgressBar::new();
    req_progress_bar.set_visible(false);
    req_progress_bar.set_show_text(true);

    let accept_button = Button::with_label("Accept");
    accept_button.add_css_class("suggested-action");
    accept_button.add_css_class("pill");

    let decline_button = Button::with_label("Decline");
    decline_button.add_css_class("destructive-action");
    decline_button.add_css_class("pill");

    let req_cancel_button = Button::with_label("Cancel");
    req_cancel_button.add_css_class("destructive-action");
    req_cancel_button.add_css_class("pill");
    req_cancel_button.set_visible(false);

    page3_box.append(&req_icon);
    page3_box.append(&req_title);
    page3_box.append(&req_device_label);
    page3_box.append(&req_files_label);
    page3_box.append(&req_progress_bar);
    page3_box.append(&accept_button);
    page3_box.append(&decline_button);
    page3_box.append(&req_cancel_button);

    stack.add_named(&page3_box, Some("request"));
    content.append(&stack);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Send Files")
        .content(&content)
        .default_width(400)
        .default_height(500)
        .build();

    let selected_file_path = Arc::new(Mutex::new(None::<PathBuf>));
    let current_transfer_id = Arc::new(Mutex::new(None::<String>));

    // Handle incoming events from the background service
    APP_STATE.with(|state| {
        if let Some(app_state) = state.borrow().as_ref() {
            let receiver = app_state.event_receiver.clone();
            let stack_weak = stack.downgrade();
            let req_device_weak = req_device_label.downgrade();
            let req_files_weak = req_files_label.downgrade();
            let req_progress_weak = req_progress_bar.downgrade();
            let outbound_progress_weak = progress_bar.downgrade();
            let transfer_id_weak = Arc::downgrade(&current_transfer_id);
            let devices_flowbox_weak = devices_flowbox.downgrade();
            let selected_file_path_weak = Arc::downgrade(&selected_file_path);
            let accept_button_weak = accept_button.downgrade();
            let decline_button_weak = decline_button.downgrade();
            let req_cancel_button_weak = req_cancel_button.downgrade();

            glib::MainContext::default().spawn_local(async move {
                while let Ok(event) = receiver.recv().await {
                    match event {
                        ServiceEvent::EndpointFound(endpoint) => {
                            println!("DEBUG: UI Event - EndpointFound: {:?}", endpoint.id);
                            if let Some(fb) = devices_flowbox_weak.upgrade() {
                                if let Some(path) = selected_file_path_weak.upgrade() {
                                    add_device_to_ui(&fb, endpoint, path);
                                }
                            }
                        }
                        ServiceEvent::InboundRequest { id, device_name, files_count, total_bytes } => {
                            println!("DEBUG: UI Event - InboundRequest: {}", id);
                            if let Some(s) = stack_weak.upgrade() {
                                if let Some(dl) = req_device_weak.upgrade() {
                                    dl.set_text(&format!("From: {}", device_name));
                                }
                                if let Some(fl) = req_files_weak.upgrade() {
                                    let size_str = if total_bytes > 1024 * 1024 {
                                        format!("{:.2} MB", total_bytes as f64 / (1024.0 * 1024.0))
                                    } else {
                                        format!("{:.2} KB", total_bytes as f64 / 1024.0)
                                    };
                                    fl.set_text(&format!("{} file(s) ({})", files_count, size_str));
                                }
                                if let Some(tid) = transfer_id_weak.upgrade() {
                                    *tid.lock().unwrap() = Some(id);
                                }
                                if let Some(pb) = req_progress_weak.upgrade() {
                                    pb.set_visible(false);
                                    pb.set_fraction(0.0);
                                }
                                if let Some(ab) = accept_button_weak.upgrade() { ab.set_visible(true); }
                                if let Some(db) = decline_button_weak.upgrade() { db.set_visible(true); }
                                if let Some(rcb) = req_cancel_button_weak.upgrade() { rcb.set_visible(false); }
                                s.set_visible_child_name("request");
                            }
                        }
                        ServiceEvent::OutboundStarted(id) => {
                            println!("DEBUG: UI Event - OutboundStarted: {}", id);
                            if let Some(tid) = transfer_id_weak.upgrade() {
                                *tid.lock().unwrap() = Some(id);
                            }
                            if let Some(pb) = outbound_progress_weak.upgrade() {
                                pb.set_visible(true);
                                pb.set_fraction(0.0);
                            }
                        }
                        ServiceEvent::Progress(progress) => {
                            let pct_text = format!("{:.0}%", progress * 100.0);
                            if let Some(pb) = req_progress_weak.upgrade() {
                                if pb.is_visible() {
                                    pb.set_fraction(progress);
                                    pb.set_text(Some(&pct_text));
                                }
                            }
                            if let Some(pb) = outbound_progress_weak.upgrade() {
                                if pb.is_visible() {
                                    pb.set_fraction(progress);
                                    pb.set_text(Some(&pct_text));
                                }
                            }
                        }
                        ServiceEvent::Finished => {
                            println!("DEBUG: UI Event - Finished");
                            if let Some(tid) = transfer_id_weak.upgrade() {
                                *tid.lock().unwrap() = None;
                            }
                            if let Some(s) = stack_weak.upgrade() {
                                if let Some(pb) = req_progress_weak.upgrade() { pb.set_visible(false); }
                                if let Some(pb) = outbound_progress_weak.upgrade() { pb.set_visible(false); }
                                s.set_visible_child_name("initial");
                            }
                        }
                        ServiceEvent::Error(err) => {
                            println!("DEBUG: UI Event - Error: {}", err);
                            if let Some(tid) = transfer_id_weak.upgrade() {
                                *tid.lock().unwrap() = None;
                            }
                            if let Some(s) = stack_weak.upgrade() {
                                if let Some(pb) = req_progress_weak.upgrade() { pb.set_visible(false); }
                                if let Some(pb) = outbound_progress_weak.upgrade() { pb.set_visible(false); }
                                s.set_visible_child_name("initial");
                            }
                        }
                    }
                }
            });
        }
    });

    // --- Signal Handling ---

    let file_chooser = gtk4::FileChooserNative::builder()
        .title("Select a File")
        .transient_for(&window)
        .action(gtk4::FileChooserAction::Open)
        .modal(true)
        .build();

    send_button.connect_clicked(glib::clone!(#[strong] file_chooser, move |_| {
        file_chooser.show();
    }));

    file_chooser.connect_response(glib::clone!(#[weak] stack, #[weak] file_name_label, #[strong] selected_file_path, #[weak] devices_flowbox, move |d, response| {
        if response == gtk4::ResponseType::Accept {
            if let Some(file) = d.file() {
                 let name = file.basename().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "Unknown".to_string());
                 file_name_label.set_text(&truncate_filename(&name, 30));
                 
                 if let Some(path) = file.path() {
                     *selected_file_path.lock().unwrap() = Some(path);
                 }

                 stack.set_visible_child_name("selected");
                 
                 // Start Discovery when entering this page
                 start_discovery(&devices_flowbox, selected_file_path.clone());
            }
        }
    }));


    cancel_button.connect_clicked(glib::clone!(#[strong] current_transfer_id, #[weak] stack, #[weak] progress_bar, move |_| {
        let tid_opt = current_transfer_id.lock().unwrap().clone();
        if let Some(id) = tid_opt {
             APP_STATE.with(|state| {
                 if let Some(app_state) = state.borrow().as_ref() {
                     let msg_sender_opt = app_state.msg_sender.lock().unwrap().clone();
                     if let Some(sender) = msg_sender_opt {
                         let _ = sender.send(rqs_lib::channel::ChannelMessage {
                             id,
                             msg: rqs_lib::channel::Message::Lib {
                                 action: rqs_lib::channel::TransferAction::TransferCancel,
                             },
                         });
                     }
                 }
             });
        }
        *current_transfer_id.lock().unwrap() = None;
        println!("DEBUG: Outbound transfer manually canceled and state cleared");
        progress_bar.set_visible(false);
        stack.set_visible_child_name("initial");
    }));

    accept_button.connect_clicked(glib::clone!(#[strong] current_transfer_id, #[weak] req_progress_bar, #[weak] accept_button, #[weak] decline_button, #[weak] req_cancel_button, move |_| {
        let tid_opt = current_transfer_id.lock().unwrap().clone();
        if let Some(id) = tid_opt {
            req_progress_bar.set_visible(true);
            accept_button.set_visible(false);
            decline_button.set_visible(false);
            req_cancel_button.set_visible(true);
            APP_STATE.with(|state| {
                if let Some(app_state) = state.borrow().as_ref() {
                    let msg_sender_opt = app_state.msg_sender.lock().unwrap().clone();
                    if let Some(sender) = msg_sender_opt {
                        let _ = sender.send(rqs_lib::channel::ChannelMessage {
                            id,
                            msg: rqs_lib::channel::Message::Lib {
                                action: rqs_lib::channel::TransferAction::ConsentAccept,
                            },
                        });
                    }
                }
            });
        }
    }));

    decline_button.connect_clicked(glib::clone!(#[strong] current_transfer_id, #[weak] stack, move |_| {
        let tid_opt = current_transfer_id.lock().unwrap().clone();
        if let Some(id) = tid_opt {
            APP_STATE.with(|state| {
                if let Some(app_state) = state.borrow().as_ref() {
                    let msg_sender_opt = app_state.msg_sender.lock().unwrap().clone();
                    if let Some(sender) = msg_sender_opt {
                        let _ = sender.send(rqs_lib::channel::ChannelMessage {
                            id,
                            msg: rqs_lib::channel::Message::Lib {
                                action: rqs_lib::channel::TransferAction::ConsentDecline,
                            },
                        });
                    }
                }
            });
        }
        stack.set_visible_child_name("initial");
    }));

    req_cancel_button.connect_clicked(glib::clone!(#[strong] current_transfer_id, #[weak] stack, move |_| {
        let tid_opt = current_transfer_id.lock().unwrap().clone();
        if let Some(id) = tid_opt {
            APP_STATE.with(|state| {
                if let Some(app_state) = state.borrow().as_ref() {
                    let msg_sender_opt = app_state.msg_sender.lock().unwrap().clone();
                    if let Some(sender) = msg_sender_opt {
                        let _ = sender.send(rqs_lib::channel::ChannelMessage {
                            id,
                            msg: rqs_lib::channel::Message::Lib {
                                action: rqs_lib::channel::TransferAction::TransferCancel,
                            },
                        });
                    }
                }
            });
        }
        *current_transfer_id.lock().unwrap() = None;
        println!("DEBUG: Inbound transfer manually canceled and state cleared");
        stack.set_visible_child_name("initial");
    }));

    window.present();
}

fn start_discovery(_flowbox: &FlowBox, _selected_file_path: Arc<Mutex<Option<PathBuf>>>) {
    // Ya no borramos la lista. Queremos que los dispositivos descubiertos persistan.
    println!("DEBUG: Descubrimiento activo (esperando eventos mDNS)");
}

fn add_device_to_ui(flowbox: &FlowBox, endpoint: rqs_lib::EndpointInfo, selected_file: Arc<Mutex<Option<PathBuf>>>) {
    let endpoint_id = endpoint.id.clone();
    let name = endpoint.name.as_deref().unwrap_or("Unknown");
    
    // Buscar si ya existe el dispositivo por ID o nombre
    let mut current_child = flowbox.first_child();
    while let Some(child) = current_child {
        let existing_id = child.widget_name();
        
        if existing_id.contains(&endpoint_id) {
            // Si el nombre actual es "Unknown" pero el nuevo es real, borramos el "Unknown" para añadir el bueno
            if existing_id.contains("Unknown") && name != "Unknown" {
                println!("DEBUG: Reemplazando dispositivo 'Unknown' por nombre real: {}", name);
                flowbox.remove(&child);
                break; 
            }
            println!("DEBUG: Saltando dispositivo duplicado o menos detallado: {} ({})", name, endpoint_id);
            return;
        }
        current_child = child.next_sibling();
    }

    println!("DEBUG: Añadiendo dispositivo a la lista: {} ({})", name, endpoint_id);

    // Create UI element
    let initials = name.chars().next().unwrap_or('?').to_uppercase().to_string();
    
    let avatar = Avatar::builder()
        .text(&initials)
        .size(60)
        .show_initials(true)
        .build();
    
    let label = gtk4::Label::new(Some(name));
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label.set_max_width_chars(10);
    
    let container = Box::new(Orientation::Vertical, 4);
    container.append(&avatar);
    container.append(&label);
    
    let button = Button::builder()
        .child(&container)
        .has_frame(false)
        .build();
    
    let endpoint_name = endpoint.name.clone().unwrap_or_default();
    let endpoint_ip = endpoint.ip.clone().unwrap_or_default();
    let endpoint_port = endpoint.port.unwrap_or_default();

    button.connect_clicked(move |_| {
        println!("Click en dispositivo: {}", endpoint_id);
        
        let file_path_opt = selected_file.lock().unwrap().clone();
        if let Some(path) = file_path_opt {
             APP_STATE.with(|state| {
                 if let Some(app_state) = state.borrow().as_ref() {
                      let runtime = &app_state.runtime;
                      let file_sender_mutex = app_state.file_sender.clone();
                      
                      let id_clone = endpoint_id.clone();
                      let name_clone = endpoint_name.clone();
                      let addr_clone = format!("{}:{}", endpoint_ip, endpoint_port);
                      let path_clone = path.to_string_lossy().to_string();

                      runtime.spawn(async move {
                          let sender_opt = { file_sender_mutex.lock().unwrap().clone() };
                          if let Some(sender) = sender_opt {
                                // Generate a unique ID for this transfer attempt
                                let unique_id = format!("{}-{}", id_clone, 
                                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
                                
                                println!("DEBUG: Starting outbound transfer {} with files: {:?}", unique_id, vec![path_clone.clone()]);
                                
                                let info = rqs_lib::SendInfo {
                                    id: unique_id,
                                    name: name_clone,
                                    addr: addr_clone,
                                    ob: rqs_lib::OutboundPayload::Files(vec![path_clone]),
                                };
                                if let Err(e) = sender.send(info).await {
                                    println!("DEBUG: Failed to send: {:?}", e);
                                }
                          }
                      });
                 }
             });
        }
    });

    let row = gtk4::FlowBoxChild::new();
    row.set_child(Some(&button));
    // Set a composite ID for robust lookup
    row.set_widget_name(&format!("{}:::{}", endpoint.id, name));
    
    flowbox.insert(&row, -1);
}

fn truncate_filename(name: &str, max_len: usize) -> String {
    if name.len() <= max_len {
        return name.to_string();
    }

    let path = std::path::Path::new(name);
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);

    if stem.len() <= 15 {
        return name.to_string();
    }

    let truncated_stem = &stem[..15];
    if !extension.is_empty() {
        format!("{} ... .{}", truncated_stem, extension)
    } else {
        format!("{} ...", truncated_stem)
    }
}
