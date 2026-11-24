// A first plugin. Everything outside the functions runs once, when the plugin starts.

// a button in the header (an icon; the label shows as a tooltip)
ui::button(#{ id: "wave", label: "Say hello", icon: "sparkle", place: "header" });

// an entry in the menu of every song (right click)
ui::menu_item(#{ id: "count", label: "Count this song", icon: "star" });

// a settings page of its own
settings::category("hello", "Hello", "sparkle");
settings::heading("hello", "Notifications");
let announce = settings::toggle("hello", "announce", "Announce every new song", "Shows a small notification when the song changes.", true);
let greeting = settings::text("hello", "greeting", "Greeting", "Used by the header button.", "Hello");
settings::label("hello", "counted", "Songs counted: " + (storage::get("counted") ?? 0));

// the song changed (`track` has id, title, artist, artists, album, duration, explicit, cover)
fn on_track(track) {
    if settings::get("announce") {
        ui::toast("Now playing " + track.title + " by " + track.artist);
    }
}

// a button, menu entry or settings button was used
fn on_action(id, data) {
    if id == "wave" {
        ui::toast(settings::get("greeting") + "!");
    } else if id == "count" {
        let n = (storage::get("counted") ?? 0) + 1;
        storage::set("counted", n);
        settings::set_label("counted", "Songs counted: " + n);
        ui::toast("Counted “" + data.track.title + "” (" + n + ")");
    }
}
