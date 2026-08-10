pub mod account;
pub mod auth;
pub mod candidates;
pub mod debug;
pub mod elections;
pub mod homepage;
pub mod info;
pub mod login;
pub mod manage_election_status;
pub mod manage_elections;
pub mod settings;
pub mod themes;

pub use crate::backend::tasks::login_threads;

pub use account::{
    get_account_page, post_account_theme, post_delete_account_session,
        post_account_role, post_delete_all_account_sessions,
};
pub use candidates::{
    get_candidate_registration, get_election_candidates, post_candidate_registration,
    post_withdraw_candidate,
};
pub use debug::{get_debug, post_debug};
pub use elections::get_elections;
pub use homepage::get_homepage;
pub use info::{get_about, get_contact, get_staging};
pub use login::{
    get_login, get_login_oauth, get_login_oauth_callback, get_login_oauth_complete,
    get_login_oauth_device, get_login_oauth_manual_check, get_login_oauth_status, get_logout,
    get_userinfo,
};
pub use manage_election_status::{
    get_election_changes, get_manage_election_status, post_election_status,
    post_manage_council_candidate, post_manage_presidential_ticket,
};
pub use manage_elections::{
    get_edit_election, get_manage_elections, post_edit_election, post_manage_elections,
};
pub use settings::{get_settings, post_settings};
pub use themes::get_list_themes_page;
