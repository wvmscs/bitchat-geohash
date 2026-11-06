// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Entry point of the browser build (`wasm32`), for the page's worker
//! (`web/worker.js`): an [`Engine`] runs the monitor and the map's service,
//! answers the page's requests and streams its events. The worker keeps the
//! store's snapshot in the browser's database.

use std::cell::RefCell;
use std::rc::Rc;

use tokio::sync::oneshot;
use wasm_bindgen::prelude::*;

use crate::monitor::Monitor;
use crate::service::{Relaunch, Service, ServiceConfig};
use crate::store::Store;

/// The monitor and the map's service, in the browser.
#[wasm_bindgen]
pub struct Engine {
    service: Rc<Service>,
}

fn js_error(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

#[wasm_bindgen]
impl Engine {
    /// Starts on `args`, the options of the command line (`["u09t",
    /// "--depth", "2"]`). `snapshot`: the store saved last time, if any;
    /// `save(snapshot)` is called with each new one.
    pub async fn start(
        args: Vec<String>,
        snapshot: Option<String>,
        save: js_sys::Function,
    ) -> Result<Engine, JsError> {
        let verbose = args.iter().any(|a| a == "--verbose");
        console::install(if verbose {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        });
        let parsed = crate::cli::parse(args, |flag: &str, _: &mut crate::cli::Values| {
            Ok(flag == "--verbose")
        })
        .map_err(|e| JsError::new(&e))?;
        let mut store = match snapshot {
            Some(s) => Store::restore(&s).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "saved state not used, starting afresh");
                Store::open_in_memory().expect("in memory")
            }),
            None => Store::open_in_memory().map_err(js_error)?,
        };
        store.set_saver(Rc::new(move |s: String| {
            if let Err(e) = save.call1(&JsValue::NULL, &JsValue::from_str(&s)) {
                tracing::warn!(error = ?e, "cannot save the state");
            }
        }));
        let config = parsed.config;
        let monitor = Monitor::start_from_sources(config.clone(), parsed.sources.clone(), store)
            .await
            .map_err(js_error)?;
        let relaunch = Relaunch::Sources {
            sources: parsed.sources,
            db: String::new(),
        };
        let service = Service::start(monitor, &config, Some(relaunch), ServiceConfig::default())
            .await
            .map_err(js_error)?;
        Ok(Engine {
            service: Rc::new(service),
        })
    }

    /// Answers a request of the page: a promise of `[status, body]`, the
    /// body JSON text.
    pub fn handle(&self, method: String, path: String, body: String) -> js_sys::Promise {
        let service = self.service.clone();
        wasm_bindgen_futures::future_to_promise(async move {
            let (status, body) = service.handle(&method, &path, &body).await;
            Ok(js_sys::Array::of2(&JsValue::from(status), &JsValue::from_str(&body)).into())
        })
    }

    /// The watched area now (it changes with `POST /api/area`).
    pub fn area(&self) -> String {
        self.service.area()
    }

    /// Opens an event stream: `on_message(event, id, data)` for `hello` and
    /// each delta, then `on_message("end", 0, "")` if the stream closes (the
    /// page opens another one).
    pub fn subscribe(&self, on_message: js_sys::Function) -> Subscription {
        let mut events = self.service.events();
        let (stop, mut stopped) = oneshot::channel::<()>();
        wasm_bindgen_futures::spawn_local(async move {
            let send = |event: &str, id: u64, data: &str| {
                let _ = on_message.call3(
                    &JsValue::NULL,
                    &JsValue::from_str(event),
                    &JsValue::from(id as f64),
                    &JsValue::from_str(data),
                );
            };
            loop {
                tokio::select! {
                    message = events.next() => match message {
                        Some(m) => send(&m.event, m.id, &m.data),
                        None => {
                            send("end", 0, "");
                            break;
                        }
                    },
                    _ = &mut stopped => break,
                }
            }
        });
        Subscription {
            stop: RefCell::new(Some(stop)),
        }
    }
}

/// An open event stream; [`Subscription::close`] ends it.
#[wasm_bindgen]
pub struct Subscription {
    stop: RefCell<Option<oneshot::Sender<()>>>,
}

#[wasm_bindgen]
impl Subscription {
    pub fn close(&self) {
        if let Some(stop) = self.stop.borrow_mut().take() {
            let _ = stop.send(());
        }
    }
}

/// Logs to the browser's console (the binary logs to stderr).
mod console {
    use std::fmt::{self, Write as _};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    struct Console {
        max: Level,
    }

    #[derive(Default)]
    struct Fields {
        message: String,
        rest: String,
    }

    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            if field.name() == "message" {
                let _ = write!(self.message, "{value:?}");
            } else {
                let _ = write!(self.rest, " {}={value:?}", field.name());
            }
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "message" {
                self.message.push_str(value);
            } else {
                let _ = write!(self.rest, " {}={value}", field.name());
            }
        }
    }

    impl Subscriber for Console {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            *metadata.level() <= self.max
        }

        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _: &Id, _: &Record<'_>) {}

        fn record_follows_from(&self, _: &Id, _: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields::default();
            event.record(&mut fields);
            let level = *event.metadata().level();
            let line = format!("{level} {}{}", fields.message, fields.rest);
            let line = wasm_bindgen::JsValue::from_str(&line);
            match level {
                Level::ERROR => web_sys::console::error_1(&line),
                Level::WARN => web_sys::console::warn_1(&line),
                _ => web_sys::console::log_1(&line),
            }
        }

        fn enter(&self, _: &Id) {}

        fn exit(&self, _: &Id) {}
    }

    /// Once; later calls change nothing.
    pub(super) fn install(max: Level) {
        let _ = tracing::subscriber::set_global_default(Console { max });
    }
}
