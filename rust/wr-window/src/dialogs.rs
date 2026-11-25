//! Windows' own message boxes and file pickers. Away from Windows (screenshots, tests) a question is
//! answered yes and no file is picked.
//!
//! The dialogs come from the `rfd` crate, which calls Windows' standard message box and file-open
//! dialog. They are modal and block the calling thread until the user answers; the pages call them from
//! the UI thread on a button click, so the window waits (as with any Windows dialog) but nothing else
//! in the program does.

/// Asks a yes/no question; true for Yes. Away from Windows it logs the question and answers yes.
pub fn confirm(title: &str, text: &str) -> bool {
    #[cfg(windows)]
    {
        rfd::MessageDialog::new().set_title(title).set_description(text).set_buttons(rfd::MessageButtons::YesNo).show()
            == rfd::MessageDialogResult::Yes
    }
    #[cfg(not(windows))]
    {
        wr_core::info!("{title}: {text} (yes)");
        true
    }
}

/// Shows a message with an OK button. Away from Windows it is only logged.
pub fn info(title: &str, text: &str) {
    #[cfg(windows)]
    {
        rfd::MessageDialog::new().set_title(title).set_description(text).set_buttons(rfd::MessageButtons::Ok).show();
    }
    #[cfg(not(windows))]
    {
        wr_core::info!("{title}: {text}");
    }
}

///
/// Returns the chosen file, or None if the user cancelled. Away from Windows the path comes from
/// WHITE_RABBIT_PICK_FILE (for tests), or None.
/// A file to open: (filter name, extensions).
#[allow(unused_variables)]
pub fn open_file(title: &str, filters: &[(&str, &[&str])]) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        let mut d = rfd::FileDialog::new().set_title(title);
        for (name, ext) in filters {
            d = d.add_filter(*name, ext);
        }
        d.pick_file()
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("WHITE_RABBIT_PICK_FILE").map(std::path::PathBuf::from)
    }
}
