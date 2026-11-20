//! The `/plugins` page (`docs/plugins-menu.md`): its state and keys
//! (`state`) and its view (`view`), drawn through the host's
//! [`PageKit`]. The host holds a [`PluginsPage`] while the page is open and
//! drops it when it closes.

mod host;
mod rows;
mod state;
mod view;

#[cfg(test)]
mod tests;

pub use host::{OpResult, contents, may_replace, run_op, snapshot};
pub use rows::{ListMark, PluginList, PluginRef};
pub use state::{
    AddRow, ComponentSummary, InstalledState, KeyOutcome, LineField, MarketplaceRow,
    MarketplaceSuggestion, PLUGINS_JOB_DELAY, PluginAction, PluginContents, PluginDone, PluginId,
    PluginJob, PluginOp, PluginRow, PluginsConfirm, PluginsPage, PluginsScreen, PluginsSnapshot,
    PluginsTab, SnapshotRows, plugin_actions,
};
pub use view::{KitColor, PageBuild, PageKit};
