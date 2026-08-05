pub mod account;
pub mod auth;
pub mod candidates;
pub mod debug;
pub mod homepage;
pub mod login;
pub mod manage_election_status;
pub mod manage_elections;

pub use crate::backend::tasks::login_threads;

pub use account::get_account_page;
pub use candidates::{
    get_candidate_registration, get_election_candidates, post_candidate_registration,
    post_withdraw_candidate,
};
pub use debug::{get_debug, post_debug};
pub use homepage::get_homepage;
pub use login::{
    get_login, get_login_oauth, get_login_oauth_callback, get_login_oauth_complete,
    get_login_oauth_manual_check, get_login_oauth_status, get_login_reddit, get_userinfo,
};
pub use manage_election_status::{
    get_election_changes, get_manage_election_status, post_election_status,
    post_manage_council_candidate, post_manage_presidential_ticket,
};
pub use manage_elections::{
    get_edit_election, get_manage_elections, post_edit_election, post_manage_elections,
};
