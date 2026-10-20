//! The viewer's web page, built with Trunk.

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(social_deduction_gui::view::App);
}
