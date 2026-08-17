pub mod account;
pub mod account_pages;
pub mod auth;
pub mod candidates;
pub mod census;
pub mod debug;
pub mod election_lifecycle;
pub mod elections;
pub mod homepage;
pub mod info;
pub mod login;
pub mod manage_election_status;
pub mod manage_elections;
pub mod management;
pub mod settings;
pub mod social_links;
pub mod themes;
pub mod voting;

pub use crate::backend::tasks::login_threads;

pub use account::{
    post_account_role, post_account_theme, post_delete_account_session,
    post_delete_all_account_sessions,
};
pub use account_pages::{
    get_account_appearance, get_account_overview, get_account_sessions, get_account_social,
};
pub use candidates::{
    get_candidate_registration, get_election_candidates, post_candidate_registration,
    post_withdraw_candidate,
};
pub use census::{
    get_census, get_census_month, post_activate_census, post_create_census,
    post_update_census_citizen,
};
pub use debug::{get_debug, post_debug};
pub use elections::{get_election, get_elections};
pub use homepage::get_homepage;
pub use info::{get_about, get_contact, get_staging, get_issues};
pub use login::{
    get_login, get_login_oauth, get_login_oauth_callback, get_login_oauth_complete,
    get_login_oauth_device, get_login_oauth_manual_check, get_login_oauth_status, get_logout,
    get_userinfo,
};
pub use manage_election_status::{
    get_election_changes, get_manage_election_candidates, get_manage_election_status,
    post_election_status, post_manage_council_candidate, post_manage_presidential_ticket,
};
pub use manage_elections::{
    get_edit_election, get_manage_election, get_manage_elections, get_new_election,
    post_edit_election, post_manage_elections,
};
pub use management::get_management;
pub use settings::{get_settings, post_settings, post_timezone};
pub use social_links::{
    get_discord_callback, get_discord_link, get_reddit_link, post_discord_unlink,
    post_reddit_unlink,
};
pub use themes::get_list_themes_page;
pub use voting::{get_vote, get_voter_code, post_complete_vote, post_vote, post_voter_code};
