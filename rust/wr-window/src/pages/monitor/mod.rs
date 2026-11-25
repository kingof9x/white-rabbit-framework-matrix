//! System monitor: the layout editor, where each panel's apps are placed and set up.
//!
//! The monitor (WhiteRabbit.exe) shows "apps" on each panel (a CPU bar, a clock, ...), rotating between
//! them. Which apps go where, and their options, live in the layout file the monitor reads. This module
//! is the window's editor for that file, split into four parts:
//! `catalog` lists every app the editor knows (names, sizes, settings) and makes new layout entries;
//! `model` holds the layout as pages, converts it to and from the layout file, checks it and saves it;
//! `readings` draws the live preview with the monitor's own drawing code, using this computer's CPU,
//! memory, disk and network figures and made-up sensor values;
//! `editor` connects the page's callbacks (`ui/pages/monitor.slint`) to all of the above.

pub mod catalog;
pub mod editor;
pub mod model;
pub mod readings;
