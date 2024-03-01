// Copyright The pipewire-rs Contributors.
// SPDX-License-Identifier: MIT

use pipewire as pw;
use std::{cell::RefCell, collections::HashMap};
use std::{rc::Rc, sync::Arc};

use pw::link::Link;
use pw::proxy::{Listener, ProxyListener, ProxyT};
use pw::types::ObjectType;
use pw::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Weak;

struct Proxies {
    proxies_t: HashMap<u32, Box<dyn ProxyT>>,
    listeners: HashMap<u32, Vec<Box<dyn Listener>>>,
}

impl Proxies {
    fn new() -> Self {
        Self {
            proxies_t: HashMap::new(),
            listeners: HashMap::new(),
        }
    }

    fn add_proxy_t(&mut self, proxy_t: Box<dyn ProxyT>, listener: Box<dyn Listener>) {
        let proxy_id = {
            let proxy = proxy_t.upcast_ref();
            proxy.id()
        };

        self.proxies_t.insert(proxy_id, proxy_t);

        let v = self.listeners.entry(proxy_id).or_insert_with(Vec::new);
        v.push(listener);
    }

    fn add_proxy_listener(&mut self, proxy_id: u32, listener: ProxyListener) {
        let v = self.listeners.entry(proxy_id).or_insert_with(Vec::new);
        v.push(Box::new(listener));
    }

    fn remove(&mut self, proxy_id: u32) {
        self.proxies_t.remove(&proxy_id);
        self.listeners.remove(&proxy_id);
    }
}

fn monitor(
    shared1: Weak<AtomicBool>,
    pw_receiver: pipewire::channel::Receiver<()>,
) -> Result<(), Error> {
    let shared2 = shared1.clone();

    let main_loop = pw::MainLoop::new()?;
    let _receiver = pw_receiver.attach(&main_loop, {
        let main_loop = main_loop.clone();
        move |_| main_loop.quit()
    });

    let context = pw::Context::new(&main_loop)?;
    let core = context.connect(None)?;

    let registry = Arc::new(core.get_registry()?);
    let registry_weak = Arc::downgrade(&registry);

    // Proxies and their listeners need to stay alive so store them here
    let proxies = Rc::new(RefCell::new(Proxies::new()));

    let map = Arc::new(RefCell::new(HashMap::new()));
    let map_weak = Arc::downgrade(&map);
    let map_weak2 = Arc::downgrade(&map);

    let _registry_listener = registry
        .add_listener_local()
        .global(move |obj| {
            if let Some(registry) = registry_weak.upgrade() {
                let p: Option<(Box<dyn ProxyT>, Box<dyn Listener>)> = match obj.type_ {
                    ObjectType::Link => {
                        let link: Link = registry.bind(obj).unwrap();
                        let map_super_weak = map_weak.clone();
                        let shared2 = shared1.clone();
                        let obj_listener = link
                            .add_listener_local()
                            .info(move |info| {
                                if let Some(m) = map_super_weak.upgrade() {
                                    let mut m = m.borrow_mut();
                                    match info.state() {
                                        pw::link::LinkState::Active => {
                                            m.entry(info.output_node_id()).or_insert(());
                                        }
                                        _ => {
                                            m.remove(&(info.output_node_id()));
                                        }
                                    }
                                    let old = shared2
                                        .upgrade()
                                        .unwrap()
                                        .swap(!m.is_empty(), Ordering::Relaxed);
                                    if old == m.is_empty() {
                                        println!("pipewire {}", !old);
                                    }
                                }
                            })
                            .register();
                        Some((Box::new(link), Box::new(obj_listener)))
                    }
                    _ => None,
                };

                if let Some((proxy_spe, listener_spe)) = p {
                    let proxy = proxy_spe.upcast_ref();
                    let proxy_id = proxy.id();
                    // Use a weak ref to prevent references cycle between Proxy and proxies:
                    // - ref on proxies in the closure, bound to the Proxy lifetime
                    // - proxies owning a ref on Proxy as well
                    let proxies_weak = Rc::downgrade(&proxies);

                    let listener = proxy
                        .add_listener_local()
                        .removed(move || {
                            if let Some(proxies) = proxies_weak.upgrade() {
                                proxies.borrow_mut().remove(proxy_id);
                            }
                        })
                        .register();

                    proxies.borrow_mut().add_proxy_t(proxy_spe, listener_spe);
                    proxies.borrow_mut().add_proxy_listener(proxy_id, listener);
                }
            }
        })
        .global_remove(move |id| {
            if let Some(map) = map_weak2.upgrade() {
                let mut mm = map.borrow_mut();
                mm.remove(&id);
                let old = shared2
                    .upgrade()
                    .unwrap()
                    .swap(!mm.is_empty(), Ordering::Relaxed);
                if old == mm.is_empty() {
                    println!("pipewire {}", !old);
                }
            }
        })
        .register();

    main_loop.run();
    println!("pipewire done");

    Ok(())
}

pub fn watch(
    shared1: Weak<AtomicBool>,
    pw_receiver: pipewire::channel::Receiver<()>,
) -> Result<(), Error> {
    pw::init();

    monitor(shared1, pw_receiver)?;

    unsafe {
        pw::deinit();
    }

    Ok(())
}
