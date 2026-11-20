//! Plugins: directories in the Claude Code, Codex or agent-plugins.org
//! layout, listed by marketplaces and installed into the config home, whose
//! skills, commands, agents, hooks and MCP servers join the session.
//!
//! The split is the crate's usual one. The formats are pure — ids
//! ([`id`]), where a marketplace or a plugin comes from ([`source`]), the
//! plugin manifest ([`manifest`]), the marketplace manifest ([`marketplace`]),
//! the `plugins.json` state file ([`state`]) and the catalog a page or a
//! listing shows ([`catalog`]) — as are the transforms that turn what a
//! plugin ships into what the session's own loaders read ([`vars`],
//! [`command`], [`normalize`]). The I/O sits in three places: the `git` and
//! filesystem primitives ([`git`], [`fsops`]), the reader of a plugin
//! directory ([`layout`]), and the store every operation goes through
//! ([`store`]). See `docs/plugins.md`.

pub mod catalog;
pub mod command;
pub mod fsops;
pub mod git;
pub mod id;
pub mod layout;
pub mod manifest;
pub mod marketplace;
pub mod normalize;
pub mod source;
pub mod state;
pub mod store;
pub mod vars;

// --- formats: id, source, manifest, marketplace, state, catalog -----------
// (re-exports by name; never a glob)

// --------------------------------------------------------------------------

// --- transforms and primitives: vars, command, normalize, git, fsops ------
// (re-exports by name; never a glob)

// --------------------------------------------------------------------------

// --- the store: layout, store ----------------------------------------------
// (re-exports by name; never a glob)

// --------------------------------------------------------------------------
