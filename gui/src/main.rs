#![allow(clippy::type_complexity)]

use bevy::{prelude::*, winit::WinitSettings};
use std::sync::{Arc, Condvar, Mutex};

const PLAYBACK_SAMPLE_RATE: u32 = 48000;

#[derive(Resource)]
struct MusicPlayer {
    playing: Arc<(Mutex<bool>, Condvar)>,
}

impl MusicPlayer {
    fn new() -> Self {
        let playing = Arc::new((Mutex::new(false), Condvar::new()));
        let playing_ = playing.clone();
        std::thread::spawn(move || {
            let pulse = pulse_simple::Playback::<[i16; 2]>::new(
                "phonoscule-gui",
                "GUI application for the Phonoscule music player library",
                None,
                PLAYBACK_SAMPLE_RATE,
            );

            // To begin with, let's just play a simple sine wave -- a pure tone -- using pulse-simple.
            for i in 0.. {
                {
                    let (ref lock, ref cvar) = *playing_;
                    let mut playing = lock.lock().unwrap();
                    while !*playing {
                        playing = cvar.wait(playing).unwrap();
                    }
                }

                let samples = std::array::from_fn::<_, 512, _>(|j| {
                    let freq = 60.0;
                    let vol = 0.2;
                    let x = (i * 512 + j) as f64 * freq * std::f64::consts::TAU / PLAYBACK_SAMPLE_RATE as f64;
                    let y = (x.sin() * vol * i16::MAX as f64) as i16;
                    [y, y]
                });
                pulse.write(&samples)
            }
        });
        Self { playing }
    }

    fn toggle_play_pause(&self) {
        let (ref lock, ref cvar) = *self.playing;
        let mut playing = lock.lock().unwrap();
        *playing = !*playing;
        cvar.notify_all();
        if *playing {
            println!("resumed")
        } else {
            println!("paused")
        }
    }
}

fn main() {
    App::new()
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(DefaultPlugins)
        // Only run the app when there is user input. This will significantly reduce CPU/GPU use.
        .insert_resource(WinitSettings::desktop_app())
        .insert_resource(MusicPlayer::new())
        .add_systems(Startup, setup)
        .add_systems(Update, button_system)
        .run();
}

const NORMAL_BUTTON: Color = Color::srgb(0.15, 0.15, 0.15);
const HOVERED_BUTTON: Color = Color::srgb(0.25, 0.25, 0.25);
const PRESSED_BUTTON: Color = Color::srgb(0.35, 0.75, 0.35);

fn button_system(
    mut interaction_query: Query<(&Interaction, &mut BackgroundColor, &Children), (Changed<Interaction>, With<Button>)>,
    mut text_query: Query<&mut Text>,
    mplayer: Res<MusicPlayer>,
) {
    for (interaction, mut color, children) in &mut interaction_query {
        let mut text = text_query.get_mut(children[0]).unwrap();
        match *interaction {
            Interaction::Pressed => {
                text.0 = ">.<".to_string();
                *color = PRESSED_BUTTON.into();
                mplayer.toggle_play_pause()
            }
            Interaction::Hovered => {
                text.0 = ":O".to_string();
                *color = HOVERED_BUTTON.into();
            }
            Interaction::None => {
                text.0 = "|>  ||".to_string();
                *color = NORMAL_BUTTON.into();
            }
        }
    }
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    // ui camera
    commands.spawn(Camera2d);
    commands
        .spawn((
            Button,
            Node {
                width: Val::Px(150.0),
                height: Val::Px(65.0),
                // center button
                margin: UiRect::all(Val::Auto),
                // horizontally center child text
                justify_content: JustifyContent::Center,
                // vertically center child text
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(NORMAL_BUTTON),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Button"),
                TextFont {
                    font: FontSource::Handle(asset_server.load("fonts/FiraSans-Bold.ttf")),
                    font_size: FontSize::Px(40.0),
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.9, 0.9)),
            ));
        });
}
