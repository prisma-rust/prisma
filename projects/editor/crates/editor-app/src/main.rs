#![deny(warnings)]

fn main() {
    prism_engine::util::timing::init();
    editor_app::build_app().run();
}
