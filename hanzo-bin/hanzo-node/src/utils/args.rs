pub struct Args {
    pub create_message: bool,
    pub code_registration: Option<String>,
    pub receiver_encryption_pk: Option<String>,
    pub recipient: Option<String>,
    pub _other: Option<String>,
    pub sender_subidentity: Option<String>,
    pub receiver_subidentity: Option<String>,
    pub inbox: Option<String>,
    pub body_content: Option<String>,
}

pub fn parse_args() -> Args {
    // Legacy args parsing - now handled by src/cli/mod.rs
    // Return defaults for backward compatibility
    Args {
        create_message: false,
        code_registration: None,
        receiver_encryption_pk: None,
        recipient: None,
        _other: None,
        sender_subidentity: None,
        receiver_subidentity: None,
        inbox: None,
        body_content: None,
    }
}

#[allow(dead_code)]
fn _parse_args_legacy() -> Args {
    let matches = clap::Command::new("Hanzo Node")
        .version(env!("CARGO_PKG_VERSION"))
        .arg(
            clap::Arg::new("create_message")
                .short('c')
                .long("create_message")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("code_registration")
                .short('d')
                .long("code_registration")
                .action(clap::ArgAction::Set),
        )
        .arg(
            clap::Arg::new("receiver_encryption_pk")
                .short('e')
                .long("receiver_encryption_pk")
                .action(clap::ArgAction::Set),
        )
        .arg(
            clap::Arg::new("recipient")
                .short('r')
                .long("recipient")
                .action(clap::ArgAction::Set),
        )
        .arg(clap::Arg::new("other").short('o').long("other").action(clap::ArgAction::Set))
        .arg(
            clap::Arg::new("sender_subidentity")
                .short('s')
                .long("sender_subidentity")
                .action(clap::ArgAction::Set),
        )
        .arg(
            clap::Arg::new("receiver_subidentity")
                .short('a')
                .long("receiver_subidentity")
                .action(clap::ArgAction::Set),
        )
        .arg(clap::Arg::new("inbox").short('i').long("inbox").action(clap::ArgAction::Set))
        .arg(
            clap::Arg::new("body_content")
                .short('b')
                .long("body_content")
                .action(clap::ArgAction::Set),
        )
        .get_matches();

    Args {
        create_message: matches.get_flag("create_message"),
        code_registration: matches.get_one::<String>("code_registration").cloned(),
        receiver_encryption_pk: matches.get_one::<String>("receiver_encryption_pk").cloned(),
        recipient: matches.get_one::<String>("recipient").cloned(),
        _other: matches.get_one::<String>("other").cloned(),
        sender_subidentity: matches.get_one::<String>("sender_subidentity").cloned(),
        receiver_subidentity: matches.get_one::<String>("receiver_subidentity").cloned(),
        inbox: matches.get_one::<String>("inbox").cloned(),
        body_content: matches.get_one::<String>("body_content").cloned(),
    }
}
