//! Windows' own message boxes and file pickers. Away from Windows (screenshots, tests) a question is
//! answered yes and no file is picked.

pub fn confirm(title: &str, text: &str) -> bool {
    #[cfg(windows)]
    {
        rfd::MessageDialog::new()
            .set_title(title)
            .set_description(text)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes
    }
    #[cfg(not(windows))]
    {
        wr_core::info!("{title}: {text} (yes)");
        true
    }
}

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

#[allow(unused_variables)]
pub fn save_file(title: &str, name: &str, filters: &[(&str, &[&str])]) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        let mut d = rfd::FileDialog::new().set_title(title).set_file_name(name);
        for (n, ext) in filters {
            d = d.add_filter(*n, ext);
        }
        d.save_file()
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("WHITE_RABBIT_SAVE_FILE").map(std::path::PathBuf::from)
    }
}
