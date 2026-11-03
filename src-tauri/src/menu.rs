use tauri::menu::{Menu, MenuItem, MenuItemKind};

pub(crate) const QUIT: &str = "graceful-quit";

pub(crate) fn create(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::default(app)?;
    // In the pinned Tauri default, the application submenu ends in the native
    // Quit item. macOS terminate: bypasses ExitRequested, so replace that one
    // item with a regular menu action that enters our asynchronous drain path.
    let Some(MenuItemKind::Submenu(application)) = menu.items()?.into_iter().next() else {
        return Err(std::io::Error::other("应用菜单不可用").into());
    };
    let Some(quit) = application.items()?.into_iter().next_back() else {
        return Err(std::io::Error::other("应用退出菜单不可用").into());
    };
    application.remove(&quit)?;
    application.append(&MenuItem::with_id(
        app,
        QUIT,
        format!("Quit {}", app.package_info().name),
        true,
        Some("CmdOrCtrl+Q"),
    )?)?;
    Ok(menu)
}
