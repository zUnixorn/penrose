//! Metadata around X clients and manipulating them
use crate::core::{
    data_types::Region,
    xconnection::{Atom, Prop, WmHints, XClientProperties, Xid},
};

/**
 * Meta-data around a client window that we are handling.
 *
 * Primarily state flags and information used when determining which clients
 * to show for a given monitor and how they are tiled.
 */
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct Client {
    id: Xid,
    wm_name: String,
    wm_class: String,
    window_type: String,
    workspace: usize,
    geom: Region,
    // state flags
    pub(crate) accepts_focus: bool,
    pub(crate) floating: bool,
    pub(crate) fullscreen: bool,
    pub(crate) mapped: bool,
    pub(crate) urgent: bool,
    pub(crate) wm_managed: bool,
}

impl Client {
    pub(crate) fn new<X>(conn: &X, id: Xid, workspace: usize, floating_classes: &[&str]) -> Self
    where
        X: XClientProperties,
    {
        let accepts_focus = match conn.get_prop(id, Atom::WmHints.as_ref()) {
            Ok(Prop::WmHints(WmHints { accepts_input, .. })) => accepts_input,
            _ => true,
        };

        let geom = match conn.get_prop(id, Atom::WmNormalHints.as_ref()) {
            Ok(Prop::WmNormalHints(nh)) => nh.requested_position(),
            _ => None,
        }
        .or(Some(Region::default()))
        .unwrap();

        let wm_name = conn.client_name(id).unwrap_or("unknown".into());

        let wm_class = match conn.get_prop(id, Atom::WmClass.as_ref()) {
            Ok(Prop::UTF8String(strs)) => strs[0].clone(),
            _ => "".into(),
        };

        let window_type = match conn.get_prop(id, Atom::NetWmWindowType.as_ref()) {
            Ok(Prop::UTF8String(strs)) => strs[0].clone(),
            _ => "".into(),
        };

        let floating = conn.client_should_float(id, floating_classes);

        Self {
            id,
            wm_name,
            wm_class,
            window_type,
            workspace,
            geom,
            accepts_focus,
            floating,
            fullscreen: false,
            mapped: false,
            urgent: false,
            wm_managed: true,
        }
    }

    /// The X window ID of this client
    pub fn id(&self) -> Xid {
        self.id
    }

    /// The WM_CLASS property of this client
    pub fn wm_class(&self) -> &str {
        &self.wm_class
    }

    /// The WM_NAME property of this client
    pub fn wm_name(&self) -> &str {
        &self.wm_name
    }

    /// Whether or not this client is currently fullscreen
    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// The current workspace index that this client is showing on
    pub fn workspace(&self) -> usize {
        self.workspace
    }

    /// Mark this window as being on a new workspace
    pub fn set_workspace(&mut self, workspace: usize) {
        self.workspace = workspace
    }

    /// Set the floating state of this client
    pub fn set_floating(&mut self, floating: bool) {
        self.floating = floating
    }

    pub(crate) fn set_name(&mut self, name: impl Into<String>) {
        self.wm_name = name.into()
    }

    /// The WM_CLASS of the window that this Client is tracking
    pub fn class(&self) -> &str {
        &self.wm_class
    }

    /// Mark this client as not being managed by the WindowManager directly
    pub fn externally_managed(&mut self) {
        self.wm_managed = false;
    }

    /// Mark this client as being managed by the WindowManager directly
    pub fn internally_managed(&mut self) {
        self.wm_managed = true;
    }
}
