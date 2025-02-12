//! Core data structures and user facing functionality for the window manager
use crate::{
    pure::{Diff, StackSet, Workspace},
    x::{manage_without_refresh, Atom, Prop, XConn, XConnExt, XEvent},
    Color, Error, Result,
};
use anymap::{any::Any, AnyMap};
use nix::sys::signal::{signal, SigHandler, Signal};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::{
    any::TypeId,
    cell::RefCell,
    collections::{HashMap, HashSet},
    fmt,
    ops::Deref,
    sync::Arc,
};
use tracing::{error, info, span, trace, Level};

pub mod bindings;
pub(crate) mod handle;
pub mod hooks;
pub mod layout;

use bindings::{KeyBindings, MouseBindings};
use hooks::{EventHook, ManageHook, StateHook};
use layout::LayoutStack;

/// An X11 ID for a given resource
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Default, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct Xid(pub(crate) u32);

impl std::fmt::Display for Xid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for Xid {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<u32> for Xid {
    fn from(id: u32) -> Self {
        Self(id)
    }
}

impl From<Xid> for u32 {
    fn from(id: Xid) -> Self {
        id.0
    }
}

/// The pure client state information for the window manager
pub type ClientSet = StackSet<Xid>;

/// The pure client state information for a single [Workspace]
pub type ClientSpace = Workspace<Xid>;

/// Mutable internal state for the window manager
#[derive(Debug)]
pub struct State<X>
where
    X: XConn,
{
    /// The user defined configuration options for running the main window manager logic
    pub config: Config<X>,
    /// The pure window manager state
    pub client_set: ClientSet,
    pub(crate) extensions: AnyMap,
    pub(crate) root: Xid,
    pub(crate) mapped: HashSet<Xid>,
    pub(crate) pending_unmap: HashMap<Xid, usize>,
    pub(crate) current_event: Option<XEvent>,
    pub(crate) diff: Diff<Xid>,
    // pub(crate) mouse_focused: bool,
    // pub(crate) mouse_position: Option<(Point, Point)>,
}

impl<X> State<X>
where
    X: XConn,
{
    /// The Xid of the root window for the running [WindowManager].
    pub fn root(&self) -> Xid {
        self.root
    }

    /// The set of all client windows currently mapped to a screen.
    pub fn mapped_clients(&self) -> &HashSet<Xid> {
        &self.mapped
    }

    /// The event currently being processed.
    pub fn current_event(&self) -> Option<&XEvent> {
        self.current_event.as_ref()
    }

    /// Get access to a shared state extension.
    ///
    /// To add an extension to [State] before starting the Window Manager, see the
    /// [WindowManager::add_extension] method. To add an extension dynamically
    /// when you have access to [State], see [State::add_extension].
    ///
    /// # Errors
    /// Returns `Error::UnknownStateExtension` if there is no extension of type `E`.
    pub fn extension<E: Any>(&self) -> Result<Arc<RefCell<E>>> {
        self.extensions
            .get()
            .map(Arc::clone)
            .ok_or(Error::UnknownStateExtension {
                type_id: TypeId::of::<E>(),
            })
    }

    /// Get access to a shared state extension or set it using Default.
    pub fn extension_or_default<E: Default + Any>(&mut self) -> Arc<RefCell<E>> {
        if !self.extensions.contains::<Arc<RefCell<E>>>() {
            self.add_extension(E::default());
        }

        self.extension().expect("to have defaulted if missing")
    }

    /// Remove a shared state extension entirely.
    ///
    /// Returns `None` if there is no extension of type `E` or if that extension
    /// is currently being held by another thread.
    pub fn remove_extension<E: Any>(&mut self) -> Option<E> {
        let arc: Arc<RefCell<E>> = self.extensions.remove()?;

        // If there is only one strong reference to this state then we'll be able to
        // try_unwrap it and return the underlying `E`. If not the this fails so we
        // need to store it back in the extensions anymap.
        match Arc::try_unwrap(arc) {
            Ok(rc) => Some(rc.into_inner()),
            Err(arc) => {
                self.extensions.insert(arc);
                None
            }
        }
    }

    /// Add a typed [State] extension to this State.
    pub fn add_extension<E: Any>(&mut self, extension: E) {
        self.extensions.insert(Arc::new(RefCell::new(extension)));
    }
}

/// The user specified config options for how the window manager should run
pub struct Config<X>
where
    X: XConn,
{
    /// The RGBA color to use for normal (unfocused) window borders
    pub normal_border: Color,
    /// The RGBA color to use for the focused window border
    pub focused_border: Color,
    /// The width in pixels to use for drawing window borders
    pub border_width: u32,
    /// Whether or not the mouse entering a new window should set focus
    pub focus_follow_mouse: bool,
    /// The stack of layouts to use for each workspace
    pub default_layouts: LayoutStack,
    /// The ordered set of workspace tags to use on window manager startup
    pub tags: Vec<String>,
    /// Window classes that should always be assigned floating positions rather than tiled
    pub floating_classes: Vec<String>,
    /// A [StateHook] to run before entering the main event loop
    pub startup_hook: Option<Box<dyn StateHook<X>>>,
    /// A [StateHook] to run before processing each [XEvent]
    pub event_hook: Option<Box<dyn EventHook<X>>>,
    /// A [ManageHook] to run after each new window becomes managed by the window manager
    pub manage_hook: Option<Box<dyn ManageHook<X>>>,
    /// A [StateHook] to run every time the on screen X state is refreshed
    pub refresh_hook: Option<Box<dyn StateHook<X>>>,
}

impl<X> fmt::Debug for Config<X>
where
    X: XConn,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("normal_border", &self.normal_border)
            .field("focused_border", &self.focused_border)
            .field("border_width", &self.border_width)
            .field("focus_follow_mouse", &self.focus_follow_mouse)
            .field("default_layouts", &self.default_layouts)
            .field("tags", &self.tags)
            .field("floating_classes", &self.floating_classes)
            .finish()
    }
}

impl<X> Default for Config<X>
where
    X: XConn,
{
    fn default() -> Self {
        let strings = |slice: &[&str]| slice.iter().map(|s| s.to_string()).collect();

        Config {
            normal_border: "#3c3836".try_into().expect("valid hex code"),
            focused_border: "#cc241d".try_into().expect("valid hex code"),
            border_width: 2,
            focus_follow_mouse: true,
            default_layouts: LayoutStack::default(),
            tags: strings(&["1", "2", "3", "4", "5", "6", "7", "8", "9"]),
            floating_classes: strings(&["dmenu", "dunst"]),
            startup_hook: None,
            event_hook: None,
            manage_hook: None,
            refresh_hook: None,
        }
    }
}

impl<X> Config<X>
where
    X: XConn,
{
    /// Set the startup_hook or compose it with what is already set.
    ///
    /// The new hook will run before what was there before.
    pub fn compose_or_set_startup_hook<H>(&mut self, hook: H)
    where
        H: StateHook<X> + 'static,
        X: 'static,
    {
        self.startup_hook = match self.startup_hook.take() {
            Some(h) => Some(hook.then_boxed(h)),
            None => Some(hook.boxed()),
        };
    }

    /// Set the event_hook or compose it with what is already set.
    ///
    /// The new hook will run before what was there before.
    pub fn compose_or_set_event_hook<H>(&mut self, hook: H)
    where
        H: EventHook<X> + 'static,
        X: 'static,
    {
        self.event_hook = match self.event_hook.take() {
            Some(h) => Some(hook.then_boxed(h)),
            None => Some(hook.boxed()),
        };
    }

    /// Set the manage_hook or compose it with what is already set.
    ///
    /// The new hook will run before what was there before.
    pub fn compose_or_set_manage_hook<H>(&mut self, hook: H)
    where
        H: ManageHook<X> + 'static,
        X: 'static,
    {
        self.manage_hook = match self.manage_hook.take() {
            Some(h) => Some(hook.then_boxed(h)),
            None => Some(hook.boxed()),
        };
    }

    /// Set the refresh_hook or compose it with what is already set.
    ///
    /// The new hook will run before what was there before.
    pub fn compose_or_set_refresh_hook<H>(&mut self, hook: H)
    where
        H: StateHook<X> + 'static,
        X: 'static,
    {
        self.refresh_hook = match self.refresh_hook.take() {
            Some(h) => Some(hook.then_boxed(h)),
            None => Some(hook.boxed()),
        };
    }
}

/// A top level struct holding all of the state required to run as an X11 window manager.
///
/// This allows for final configuration to be carried out before entering the main event
/// loop.
#[derive(Debug)]
pub struct WindowManager<X>
where
    X: XConn,
{
    x: X,
    /// The mutable [State] of the window manager
    pub state: State<X>,
    key_bindings: KeyBindings<X>,
    mouse_bindings: MouseBindings<X>,
}

impl<X> WindowManager<X>
where
    X: XConn,
{
    /// Construct a new [WindowManager] with the provided config and X connection.
    ///
    /// If you need to set [State] extensions, call [WindowManager::add_extension] after
    /// constructing your initial WindowManager.
    pub fn new(
        config: Config<X>,
        key_bindings: KeyBindings<X>,
        mouse_bindings: MouseBindings<X>,
        x: X,
    ) -> Result<Self> {
        let mut client_set = StackSet::try_new(
            config.default_layouts.clone(),
            config.tags.iter(),
            x.screen_details()?,
        )?;

        let ss = client_set.snapshot(vec![]);
        let diff = Diff::new(ss.clone(), ss);

        let state = State {
            config,
            client_set,
            extensions: AnyMap::new(),
            root: x.root(),
            mapped: HashSet::new(),
            pending_unmap: HashMap::new(),
            current_event: None,
            diff,
        };

        Ok(Self {
            x,
            state,
            key_bindings,
            mouse_bindings,
        })
    }

    /// Add a typed [State] extension to this WindowManager.
    pub fn add_extension<E: Any>(&mut self, extension: E) {
        self.state.add_extension(extension);
    }

    /// Start the WindowManager and run it until told to exit.
    ///
    /// Any provided startup hooks will be run after setting signal handlers and grabbing
    /// key / mouse bindings from the X server. Any set up you need to do should be run
    /// explicitly before calling this method or as part of a startup hook.
    pub fn run(mut self) -> Result<()> {
        info!("registering SIGCHILD signal handler");
        if let Err(e) = unsafe { signal(Signal::SIGCHLD, SigHandler::SigIgn) } {
            panic!("unable to set signal handler: {}", e);
        }

        self.grab()?;

        if let Some(mut h) = self.state.config.startup_hook.take() {
            trace!("running user startup hook");
            if let Err(e) = h.call(&mut self.state, &self.x) {
                error!(%e, "error returned from user startup hook");
            }
        }

        self.manage_existing_clients()?;

        loop {
            match self.x.next_event() {
                Ok(event) => {
                    let span = span!(target: "penrose", Level::INFO, "XEvent", %event);
                    let _enter = span.enter();
                    trace!(details = ?event, "event details");
                    self.state.current_event = Some(event.clone());

                    if let Err(e) = self.handle_xevent(event) {
                        error!(%e, "Error handling XEvent");
                    }
                    self.x.flush();

                    self.state.current_event = None;
                }

                Err(e) => error!(%e, "Error pulling next x event"),
            }
        }
    }

    fn grab(&self) -> Result<()> {
        trace!("grabbing key and mouse bindings");
        let key_codes: Vec<_> = self.key_bindings.keys().copied().collect();
        let mouse_states: Vec<_> = self
            .mouse_bindings
            .keys()
            .map(|(_, state)| state.clone())
            .collect();

        self.x.grab(&key_codes, &mouse_states)
    }

    fn handle_xevent(&mut self, event: XEvent) -> Result<()> {
        use XEvent::*;

        let WindowManager {
            x,
            state,
            key_bindings,
            mouse_bindings,
        } = self;

        let mut hook = state.config.event_hook.take();
        let should_run = match hook {
            Some(ref mut h) => {
                trace!("running user event hook");
                match h.call(&event, state, x) {
                    Ok(should_run) => should_run,
                    Err(e) => {
                        error!(%e, "error returned from user event hook");
                        true
                    }
                }
            }

            None => true,
        };
        state.config.event_hook = hook;

        if !should_run {
            trace!("User event hook returned false: skipping default handling");
            return Ok(());
        }

        match &event {
            ClientMessage(m) => handle::client_message(m.clone(), state, x)?,
            ConfigureNotify(e) if e.is_root => handle::detect_screens(state, x)?,
            ConfigureNotify(_) => (),  // Not currently handled
            ConfigureRequest(_) => (), // Not currently handled
            Enter(p) => handle::enter(*p, state, x)?,
            Expose(_) => (), // Not currently handled
            FocusIn(id) => handle::focus_in(*id, state, x)?,
            Destroy(xid) => handle::destroy(*xid, state, x)?,
            KeyPress(code) => handle::keypress(*code, key_bindings, state, x)?,
            Leave(p) => handle::leave(*p, state, x)?,
            MappingNotify => (), // Not currently handled
            MapRequest(xid) => handle::map_request(*xid, state, x)?,
            MouseEvent(e) => handle::mouse_event(e.clone(), mouse_bindings, state, x)?,
            PropertyNotify(_) => (), // Not currently handled
            RandrNotify => handle::detect_screens(state, x)?,
            ScreenChange => handle::screen_change(state, x)?,
            UnmapNotify(xid) => handle::unmap_notify(*xid, state, x)?,
        }

        Ok(())
    }

    // A "best effort" attempt to manage existing clients on the workspaces they were present
    // on previously. This is not guaranteed to preserve the stack order or correctly handle
    // any clients that were on invisible workspaces / workspaces that no longer exist.
    //
    // NOTE: the check for if each client is already in state is in case a startup hook has
    //       pre-managed clients for us. In that case we want to avoid stomping on
    //       anything that they have set up.
    #[tracing::instrument(level = "info", skip(self))]
    fn manage_existing_clients(&mut self) -> Result<()> {
        info!("managing existing clients");

        // We're not guaranteed that workspace indices are _always_ continuous from 0..n
        // so we explicitly map tags to indices instead.
        let ws_map: HashMap<usize, String> = self
            .state
            .client_set
            .workspaces()
            .map(|w| (w.id, w.tag.clone()))
            .collect();

        let first_tag = self.state.client_set.ordered_tags()[0].clone();

        for id in self.x.existing_clients()? {
            if self.state.client_set.contains(&id)
                || self.x.get_window_attributes(id)?.override_redirect
            {
                continue;
            }

            info!(%id, "attempting to manage existing client");
            let workspace_id = match self.x.get_prop(id, Atom::NetWmDesktop.as_ref()) {
                Ok(Some(Prop::Cardinal(ids))) => ids[0] as usize,
                _ => 0, // we know that we always have at least one workspace
            };

            let tag = ws_map.get(&workspace_id).unwrap_or(&first_tag);
            manage_without_refresh(id, Some(tag), &mut self.state, &self.x)?;
        }

        info!("triggering refresh");
        self.x.refresh(&mut self.state)
    }
}
