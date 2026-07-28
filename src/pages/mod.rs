pub mod debug;
pub mod homepage;
pub mod login;

pub use debug::{get_debug, post_debug};
pub use homepage::get_homepage;
pub use login::{
    get_login,
    get_login_discord,
    get_login_discord_callback,
    get_login_discord_complete,
    get_login_discord_manual_check,
    get_login_discord_status,
    get_login_reddit,
    get_userinfo,
    login_threads,
};
