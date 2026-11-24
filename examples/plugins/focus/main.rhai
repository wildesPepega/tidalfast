// Hides parts of the interface. `app::features()` lists everything that can be hidden.
settings::category("focus", "Focus", "eye-slash");
settings::toggle("focus", "on", "Focus mode", "Hide lyrics, credits, queue and search.", false);

fn apply(on) {
    for part in ["lyrics", "credits", "queue", "search"] {
        if on { app::hide(part); } else { app::show(part); }
    }
}

// called once when the plugin has started
fn on_load() { apply(settings::get("on")); }

fn on_setting(id, value) {
    if id == "on" { apply(value); }
}
