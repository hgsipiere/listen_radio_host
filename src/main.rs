use rand::rngs::ThreadRng;
use rand::Rng;
use rodio::{OutputStream, Sink};
use serde::Deserialize;

use std::env::args;
use std::fs::read_to_string;
use std::fs::File;
use std::io::BufReader;
use std::process;

// use 'None' as ∞
fn foldn_opt<T>(f: impl Fn(T) -> T, n_opt: Option<usize>, x: T) -> T {
    let mut acc = x;
    match n_opt {
        Some(n) => {
            for _ in 0..n {
                acc = f(acc)
            }
        }
        None => loop {
            acc = f(acc)
        },
    }
    acc
}

fn foldn<T>(f: impl Fn(T) -> T, n: usize, x: T) -> T {
    foldn_opt(f, Some(n), x)
}

fn play_sound(sink: &Sink, filename: &String) -> () {
    // filepaths should not be joined like this
    // this breaks on Windows in some especially specific (theoretical) cases, to fix
    let filepath = "audio/".to_string() + filename;
    println!("{}",filepath);
    let audio_file = File::open(&filepath).expect(&("Error opening: ".to_string() + &filepath));
    let source = rodio::Decoder::new(BufReader::new(audio_file))
        .expect(&("Error decoding: ".to_string() + &filepath));
    sink.append(source);

    // The sound plays in a separate thread. This call will block the current
    // thread until the sink
    // has finished playing all its queued sounds.
    sink.sleep_until_end();
}

#[derive(Deserialize)]
struct Cfg {
    trans: Vec<String>, // radio host transition
    // some songs have a combo (song specific announcement then song)
    songs_combo: Vec<(String, Option<String>)>,
    intros: Vec<String>, // radio host introduction pieces
    min_b1_songs: usize,
    max_b1_songs: usize
}

// faulty json example, lacks at least 2 combos to avoid repeats of a combo
// {"trans":["chat.mp3","comment.mp3","interruption.mp3"],
// "songs_combo":[["t.mp3","t_c.mp3"],["a.mp3",null]],
// "intros":["hi.mp3","hola.mp3"]}

fn prs_cfg(cfg_text: String) -> Cfg {
    match serde_json::from_str(&cfg_text[..]) {
        Ok(deserialised_cfg) => deserialised_cfg,
        Err(e) => {
            eprintln! {"{}", e};
            process::exit(1)
        }
    }
}

struct Music {
    min_b1_songs : usize,
    max_b1_songs : usize,
    num_songs: usize,
    num_combos: usize,
    num_trans: usize,
    num_intros: usize,
    // we model trans: num_trans -> transition_filenames, etc
    trans: Vec<String>,
    intros: Vec<String>,
    songs: Vec<String>,
    combos: Vec<String>,
    combo_idx_to_song_idx: Vec<usize>,
    // mapping from combos to songs isn't surjective so need option for inverse
    song_idx_to_combo_idx: Vec<Option<usize>>,
}

fn cfg_to_music(cfg: Cfg) -> Music {
    let num_songs = cfg.songs_combo.len();
    let mut songs = vec![String::new(); num_songs];
    let mut combos = vec![];
    let mut combo_idx = 0;
    let mut combo_idx_to_song_idx = vec![];
    // defined on all songs, only change to set combos
    let mut song_idx_to_combo_idx = vec![None; num_songs];
    for (i, song_combo) in cfg.songs_combo.into_iter().enumerate() {
        match song_combo {
            (song, None) => songs[i] = song,
            (song, Some(combo)) => {
                songs[i] = song;
                combos.push(combo);
                combo_idx_to_song_idx.push(i);
                song_idx_to_combo_idx[i] = Some(combo_idx);
                combo_idx += 1;
            }
        };
    }

    Music {
        min_b1_songs: cfg.min_b1_songs,
        max_b1_songs: cfg.max_b1_songs,
        num_songs: num_songs,
        num_combos: combos.len(),
        num_trans: cfg.trans.len(),
        num_intros: cfg.intros.len(),
        trans: cfg.trans,
        songs: songs,
        combos: combos,
        combo_idx_to_song_idx: combo_idx_to_song_idx,
        song_idx_to_combo_idx: song_idx_to_combo_idx,
        intros: cfg.intros,
    }
}

struct State {
    music: Music,
    sink: Sink,
    lst_song_idx: usize,
    lst_trans_idx: usize,
    lst_intro_idx: usize,
    lst_combo_idx: usize,
    rng: ThreadRng,
}

// we schedule without related pieces being played consecutively, e.g. same intro
fn add_intro(mut s: State) -> State {
    let intro_offset = s.rng.gen_range(1..s.music.num_intros);
    // This is Z/#intros, so we get no repeats.
    let new_intro_idx = (s.lst_intro_idx + intro_offset) % s.music.num_intros;
    play_sound(&s.sink, &s.music.intros[new_intro_idx]);
    State {
        lst_intro_idx: new_intro_idx,
        ..s
    }
}

fn add_trans(mut s: State) -> State {
    let trans_offset = s.rng.gen_range(1..s.music.num_trans);
    let new_trans_idx = (s.lst_trans_idx + trans_offset) % s.music.num_trans;
    play_sound(&s.sink, &s.music.trans[new_trans_idx]);
    State {
        lst_trans_idx: new_trans_idx,
        ..s
    }
}

// do not schedule a combo if its respective song was played last
fn add_combo(mut s: State) -> State {
    let combo_offset = s.rng.gen_range(1..s.music.num_combos);
    let new_combo_idx = (s.lst_combo_idx + combo_offset) % s.music.num_combos;
    play_sound(&s.sink, &s.music.combos[new_combo_idx]);
    State {
        lst_combo_idx: new_combo_idx,
        lst_song_idx: s.music.combo_idx_to_song_idx[new_combo_idx],
        ..s
    }
}

// do not schedule a song if its respective combo was played last if existent
fn add_song(mut s: State) -> State {
    let song_offset = s.rng.gen_range(1..s.music.num_songs);
    let new_song_idx = (s.lst_song_idx + song_offset) % s.music.num_songs;
    play_sound(&s.sink, &s.music.songs[new_song_idx]);
    s.lst_song_idx = new_song_idx;
    match s.music.song_idx_to_combo_idx[new_song_idx] {
        None => {}
        Some(i) => s.lst_combo_idx = i,
    };
    s
}

// radio simulator scheduling algorithm
// we try to create a sense of a chatty radio host that sometimes quiets down

fn play_b1a(mut s: State) -> State {
    foldn(
        add_song,
        s.rng.gen_range(s.music.min_b1_songs..=s.music.max_b1_songs),
        add_combo(add_trans(s)),
    )
}
fn play_b1b(mut s: State) -> State {
    foldn(
        add_song,
        s.rng.gen_range(s.music.min_b1_songs..=s.music.max_b1_songs),
        add_intro(add_trans(s)),
    )
}

fn play_b1(mut s: State) -> State {
    match s.rng.gen() {
        false => play_b1a(s),
        true => play_b1b(s),
    }
}

fn play_b2(mut s: State) -> State {
    match s.rng.gen_range(0..=3 as usize) {
        0 => play_b1a(s),
        1 => play_b1b(s),
        2 => add_song(add_combo(s)),
        _ => add_song(add_intro(s)), // matches on 3
    }
}

fn main() {
    // _strm is required to be kept for some reason, breaks replaced with _
    println!("#EXTM3U");
    let (_strm, strm_handle) = OutputStream::try_default().expect("Audio error");
    let fst_arg = args().nth(1).expect("No configuration file provided");
    let play_len = args()
        .nth(2)
        .map(|x| x.parse::<usize>().expect("Poor duration"));
    let cfg_text = read_to_string(fst_arg).expect("Configuration read error");
    let mut fst_state = State {
        music: cfg_to_music(prs_cfg(cfg_text)),
        sink: Sink::try_new(&strm_handle).expect("Error getting audio sink"),
        rng: rand::thread_rng(),
        lst_intro_idx: 0,
        lst_trans_idx: 0,
        lst_song_idx: 0,
        lst_combo_idx: 0,
    };
    match fst_state.rng.gen() {
        // alternate chattiness
        false => foldn_opt(|s| play_b2(play_b1(s)), play_len, fst_state),
        true => foldn_opt(|s| play_b1(play_b2(s)), play_len, fst_state),
    };
}
