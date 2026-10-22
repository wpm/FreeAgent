//! Twenty Questions' page, built with Trunk.

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(twenty_questions_gui::view::App);
}
