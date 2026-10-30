//! The wishes the old torrent search found nothing for: the user's
//! «Хочу скачать» on 2026-10-30, «Не нашёл раздачу» for 42 of ~80.

#![allow(dead_code)]

use nikgob_core::source::{Want, WantScope};

/// (scope, artist as the wish shows it, title, album, seconds)
pub const WISHES: &[(&str, &str, &str, &str, u32)] = &[
    ("track", "9Lana", "ヨフケノパレエド - Midnight Parade", "Midnight Parade", 194),
    ("track", "Con Fetti Pesto, David Münnich, Alexx Grimm", "Meine Freundin ist kaputt", "Das Nennt Ihr Himmel (Supraland Six Inches Under Credits Song)", 188),
    ("track", "Dominic Fike", "Babydoll", "Don't Forget About Me, Demos", 98),
    ("track", "Ellie Minibot", "I Don't Want to Be an Engineer", "I Don't Want to Be an Engineer", 269),
    ("track", "Eve", "Underdog", "Underdog", 194),
    ("track", "Eve", "Wind's Anthem (feat. suis From Yorushika)", "Wind's Anthem", 223),
    ("track", "Eve", "Avant", "Kaizin", 205),
    ("track", "Gg2", "Комсомольск - Мозги & деньги //speed up//", "", 122),
    ("track", "IcyBeast", "7 Weeks & 3 Days", "7 Weeks & 3 Days", 169),
    ("track", "JVKE", "her", "her", 172),
    ("track", "LAZZY2WICE", "Summer 15/16", "Summer 15/16", 161),
    ("track", "MORGENSHTERN", "Turn It on! (feat. Palc)", "Turn It on!", 208),
    ("track", "MORGENSHTERN", "Цветок", "Цветок", 156),
    ("album", "MORGENSHTERN", "ЛЕГЕНДАРНАЯ ПЫЛЬ", "ЛЕГЕНДАРНАЯ ПЫЛЬ", 0),
    ("track", "MORGENSHTERN, ЛСП", "Зеленоглазые деффки!", "Улыбнись, дурак!", 205),
    ("track", "MellSher", "X", "X", 151),
    ("track", "MellSher, и ещё 1 исполнитель", "Разгром", "Разгром", 143),
    ("track", "Nakimushi", "Shake It Now. (feat. Ado)", "rendez-vous", 197),
    ("track", "RAT BOY, IBDY", "Who's Ready for Tomorrow", "Cyberpunk 2077: Radio, Vol. 2 (Original Soundtrack)", 117),
    ("track", "Rosa Walton, Hallie Coggins", "I Really Want to Stay at Your House", "Cyberpunk 2077: Radio, Vol. 2 (Original Soundtrack)", 247),
    ("track", "SLAVA MARLOW", "Каблуки", "ТУЗИК", 102),
    ("track", "SLAVA MARLOW", "О2", "ТУЗИК", 117),
    ("track", "SLAVA MARLOW", "Детские травмы (feat. Платина и Лера Маяк)", "ТУЗИК", 161),
    ("track", "SLAVA MARLOW", "Я в деле", "ТУЗИК", 94),
    ("track", "Yorushika", "火星人 - Martian", "second person", 233),
    ("track", "Yoshihisa Kato", "沈黙の魔女 - Chinmoku no Majo", "Secrets of the Silent Witch Original Soundtrack", 184),
    ("track", "aksfx", "Big Walk Theme (Jumping Voices)", "Big Walk Theme (Jumping Voices)", 226),
    ("album", "aksfx", "Big Walk Theme (Jumping Voices)", "Big Walk Theme (Jumping Voices)", 0),
    ("track", "emptiness", "7 weeks & 3 days (out of tune piano)", "7 weeks & 3 days (out of tune piano)", 61),
    ("track", "frutiger pm", "cloud city", "dreamcore", 139),
    ("track", "gotlibgotlibgotlib", "все хотят меня", "все хотят меня", 119),
    ("track", "gotlibgotlibgotlib, SABU", "агентство насилия™", "агентство насилия™", 135),
    ("track", "madk1d", "так похуй", "sexyswag", 108),
    ("track", "my head is empty", "personal space", "ambient vol. 12", 101),
    ("album", "mzlff", "не забытое старое", "не забытое старое", 0),
    ("track", "nyan.mp3", "Я не интроверт", "Я не интроверт", 100),
    ("album", "nyan.mp3", "nyan.pop", "nyan.pop", 0),
    ("track", "s0rrow", "stalk ur socials", "very sorrow days", 79),
    ("track", "sakanaction", "怪獣", "怪獣", 253),
    ("track", "Дайте танк (!)", "Маленький", "Маленький", 245),
    ("track", "Пошлая Молли", "Контракт", "Контракт", 205),
    ("album", "Разные исполнители", "Spider-Man: Into the Spider-Verse (Deluxe Edition / Soundtrack From & Inspired By The Motion Picture)", "Spider-Man: Into the Spider-Verse (Deluxe Edition / Soundtrack From & Inspired By The Motion Picture)", 0),
];

pub fn want_of(w: &(&str, &str, &str, &str, u32)) -> Want {
    let (scope, artist, title, album, secs) = *w;
    // The runtime takes the credit's names apart; the wish's display line
    // ("MellSher, и ещё 1 исполнитель") is what the old search sent.
    Want {
        artists: vec![artist.to_owned()],
        title: (scope == "track").then(|| title.to_owned()),
        album: (!album.is_empty()).then(|| album.to_owned()),
        scope: if scope == "album" { WantScope::Album } else { WantScope::Track },
        duration_ms: (secs > 0).then_some(secs * 1000),
        ..Want::default()
    }
}
