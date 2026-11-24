ui::button(#{ id: "copy", label: "Copy “Artist - Title”", icon: "copy", place: "player" });
settings::category("npc", "Copy now playing", "copy");
settings::toggle("npc", "block_skip", "Block the next button", "Demo of a hook: the next button does nothing while this is on.", false);

fn on_action(id, data) {
    let t = player::current();
    if t == () { ui::toast("Nothing is playing"); return; }
    system::copy(t.artist + " - " + t.title);
}

// hooks are asked before something happens; answer false to cancel it
fn hook_next() {
    !settings::get("block_skip")
}
