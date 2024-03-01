pub type Error = Box<dyn std::error::Error>;
/// An alias to Result which overrides the default Error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

use libpulse_binding::{
    context::{self, subscribe::Facility, Context, State},
    mainloop::threaded::Mainloop,
};
use std::{
    cell::RefCell,
    fmt,
    rc::Rc,
};

const PA_NAME: &str = "xidlehook";

/// See module-level docs
pub struct NotWhenAudio {
    ctx: Rc<RefCell<Context>>,
    mainloop: Rc<RefCell<Mainloop>>,
}
impl NotWhenAudio {
    /// Connect to `PulseAudio` and subscribe to notification of changes
    pub fn new<F>(sink_callbacks: F) -> Result<Self> where F: Fn(&mut Context) + 'static {
        let mainloop = Rc::new(RefCell::new(
            Mainloop::new().ok_or("pulseaudio: failed to create main loop")?,
        ));

        let ctx = Rc::new(RefCell::new(
            Context::new(&*mainloop.borrow(), PA_NAME)
                .ok_or("pulseaudio: failed to create context")?,
        ));

        // Setup context state change callback
        {
            let mainloop_ref = Rc::clone(&mainloop);
            let ctx_ref = Rc::clone(&ctx);

            ctx.borrow_mut().set_state_callback(Some(Box::new(move || {
                // Unfortunately, we need to bypass the runtime borrow
                // checker here of RefCell here, see
                // https://github.com/jnqnfe/pulse-binding-rust/issues/19
                // for details.
                let state = unsafe { &*ctx_ref.as_ptr() } // Borrow checker workaround
                    .get_state();
                match state {
                    context::State::Ready | context::State::Failed | context::State::Terminated => {
                        unsafe { &mut *mainloop_ref.as_ptr() } // Borrow checker workaround
                            .signal(false);
                    },
                    _ => {},
                }
            })));
        }

        ctx.borrow_mut()
            .connect(None, context::FlagSet::empty(), None)
            .map_err(|err| format!("pulseaudio: failed to connect context: {}", err))?;

        mainloop.borrow_mut().lock();

        if let Err(err) = mainloop.borrow_mut().start() {
            mainloop.borrow_mut().unlock();
            return Err(Error::from(format!(
                "pulseaudio: failed to start mainloop: {}",
                err
            )));
        }

        // Wait for context to be ready
        loop {
            match ctx.borrow().get_state() {
                State::Ready => {
                    break;
                },
                State::Failed | State::Terminated => {
                    mainloop.borrow_mut().unlock();
                    mainloop.borrow_mut().stop();
                    return Err("pulseaudio: context state failed/terminated unexpectedly".into());
                },
                _ => {
                    mainloop.borrow_mut().wait();
                },
            }
        }
        ctx.borrow_mut().set_state_callback(None);

sink_callbacks(&mut ctx.borrow_mut());
        // Setup notification callback
        //
        // Upon notification of a change, we will make use of introspection
        // to obtain a fresh count of active input sinks.
        {
            let ctx_ref = Rc::clone(&ctx);

            ctx.borrow_mut()
                .set_subscribe_callback(Some(Box::new(move |obj, op, _| {

                    println!("{:?} {:?}",obj,op);
                    //Some(SinkInput) Some(New)
                    //Some(SinkInput) Some(Removed)

                    let ctx_ref = unsafe { &mut *ctx_ref.as_ptr() }; // Borrow checker workaround
                    //ctx.borrow_mut().introspect().get_sink_info_by_index(index, callback)

                    sink_callbacks(ctx_ref);
                })));
        }

        // Subscribe to sink input events
        ctx.borrow_mut()
            .subscribe(Facility::SinkInput.to_interest_mask(), |_| ());

        // Check if audio is already playing
//        sink_callbacks(&mut ctx.borrow_mut());

        mainloop.borrow_mut().unlock();

        Ok(Self {
            ctx,
            mainloop,
        })
    }
}
impl fmt::Debug for NotWhenAudio {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "NotWhenAudio")
    }
}
impl Drop for NotWhenAudio {
    fn drop(&mut self) {
//        debug!("Stopping PulseAudio main loop");
        self.mainloop.borrow_mut().stop();
        self.mainloop.borrow_mut().lock();
        self.ctx.borrow_mut().disconnect();
        self.mainloop.borrow_mut().unlock();
//        debug!("Stopped");
    }
}
