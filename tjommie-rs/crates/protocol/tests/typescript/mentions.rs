//! `mentions.test.ts`

use protocol::mentions::{MentionRange, find_mentions, mention_ranges};

const BOTS: [(&str, &str); 4] = [
    ("w", "Writer"),
    ("cv", "CV Writer"),
    ("b", "Bot"),
    ("b2", "Bot 2"),
];

#[test]
fn matches_whole_names_longest_first_and_ignores_email_addresses() {
    assert_eq!(
        find_mentions("@CV Writer can you help?", &BOTS, &BOTS),
        ["cv"]
    );
    assert_eq!(
        find_mentions("@writer and @cv writer!", &BOTS, &BOTS),
        ["w", "cv"]
    );
    assert_eq!(
        find_mentions("@Bot 2 and @Bot 2 again", &BOTS, &BOTS),
        ["b2"]
    );
    assert!(find_mentions("mail me@writer.com or @Writers", &BOTS, &BOTS).is_empty());
    assert!(
        find_mentions(
            "mail me_@writer.com or me+@writer.com or @Writer_extra",
            &BOTS,
            &BOTS
        )
        .is_empty()
    );
    assert_eq!(
        find_mentions("@everyone stop", &BOTS, &BOTS),
        ["w", "cv", "b", "b2"]
    );
    let with_everyone = [BOTS[0], BOTS[1], BOTS[2], BOTS[3], ("all", "Everyone")];
    assert_eq!(
        find_mentions("@everyone stop", &with_everyone, &with_everyone),
        ["w", "cv", "b", "b2", "all"]
    );
}

#[test]
fn reads_a_rooms_mentions_against_every_bots_name_and_counts_only_the_rooms_members() {
    let room = [BOTS[2]]; // only Bot is in the room
    assert!(find_mentions("@Bot 2 please help", &BOTS, &room).is_empty()); // Bot 2 isn't taken for Bot
    assert_eq!(find_mentions("@bot please help", &BOTS, &room), ["b"]);
    assert_eq!(find_mentions("@Writer and @Bot", &BOTS, &room), ["b"]);
    assert_eq!(find_mentions("@everyone hi", &BOTS, &room), ["b"]);
}

#[test]
fn finds_where_each_mention_is_for_highlighting() {
    assert_eq!(
        mention_ranges(
            "@CV Writer add it. @everyone skip seniors.",
            &["Writer", "CV Writer"]
        ),
        [
            MentionRange {
                start: 0,
                end: 10,
                name: Some("CV Writer".into())
            },
            MentionRange {
                start: 19,
                end: 28,
                name: None
            }
        ]
    );
    assert!(mention_ranges("no mentions here", &["Writer"]).is_empty());
}
