pub mod adobe;
pub mod autostyle;
pub mod clarity_net;
pub mod clarity_net_neg;
pub mod clarity_unet;
pub mod commands;
pub mod crop;
pub mod dcp;
pub mod dngcolour;
pub mod fuji;
pub mod gradient;
pub mod gradetable;
pub mod demosaic;
pub mod detail;
pub mod develop;
pub mod denoise;
pub mod diskcache;
pub mod export;
pub mod handoff;
pub mod imagefile;
pub mod tonetable;
pub mod tools;
pub mod lens;
pub mod lensfun;
pub mod lensprofile;
pub mod mask;
pub mod keystone;
pub mod klein;
pub mod lite;
pub mod library;
pub mod removal;
pub mod patch;
pub mod mixtable;
pub mod pipeline;
pub mod prof;
pub mod paramtable;
pub mod preset;
pub mod presets;
pub mod preview;
pub mod sidecar;
pub mod subject;
pub mod transform;
pub mod memtrim;
pub mod types;
pub mod userprofiles;
pub mod zoom;
pub mod whitebalance;
pub mod xmp;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // The Cull and Retouch screens (RapidCulling / RapidRetouch modules).
        .plugin(tauri_plugin_rapidculling::init_rapidone())
        .plugin(tauri_plugin_rapidretouch::Builder::new().data_subdir("rapidretouch").build())
        .manage(commands::SceneCache::default())
        .manage(zoom::FullScene::default())
        .manage(zoom::MeasuredCache::default())
        .setup(|app| {
            // The user's own profiles (userprofiles.rs): Documents/RapidOne/Profiles,
            // a folder they can see and back up, outside the app and its build.
            use tauri::Manager;
            // Freed memory back to the system every half minute (memtrim.rs, Linux).
            crate::memtrim::start();
            // The AI models the installer carries (mask.rs::model_dirs).
            if let Ok(dir) = app.path().resource_dir() {
                crate::mask::set_resource_dir(dir);
            }
            if let Ok(dir) = app.path().document_dir() {
                crate::userprofiles::set_dir(dir.join("RapidOne").join("Profiles"));
            }
            // Lens profiles (lensfun.rs): newer Lensfun files in the app's data folder, the
            // user's own in Documents/RapidOne/Lens profiles.
            crate::lensfun::set_dirs(
                app.path().app_local_data_dir().ok().map(|d| d.join("lensfun")),
                app.path().document_dir().ok().map(|d| d.join("RapidOne").join("Lens profiles")),
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_folder,
            commands::get_full_preview,
            commands::add_photo,
            commands::trash_photos,
            commands::restore_photos,
            commands::move_to_unrated,
            commands::clear_handoffs,
            commands::mask_overlay,
            commands::lens_info,
            commands::lens_search,
            commands::lens_db_version,
            commands::lens_choose,
            commands::lens_add_profiles,
            commands::lens_names,
            commands::lens_db_save,
            commands::subject_snapshot,
            commands::find_subject,
            commands::refresh_photo,
            commands::cache_preview,
            commands::develop_preview,
            commands::render_preview,
            commands::render_handoff,
            export::export_plan,
            export::export_photo,
            export::export_workers,
            commands::auto_style_list,
            commands::auto_style_photo,
            commands::core_count,
            commands::shot_details,
            commands::auto_style_look,
            commands::auto_style_train,
            commands::remove_object,
            commands::removal_available,
            commands::can_find_objects,
            commands::find_objects,
            commands::prepare_find,
            commands::removal_hold,
            commands::removal_modes,
            commands::set_removal_mode,
            commands::remove_painted,
            commands::warm_denoise,
            commands::warm_lite,
            commands::read_flags,
            commands::rated_subfolder,
            commands::folder_views,
            commands::user_profiles,
            commands::render_detail,
            commands::add_user_profiles,
            commands::remove_user_profile,
            commands::prepare_noise,
            commands::prefetch_scene,
            commands::prepare_subject,
            commands::cached_preview,
            commands::cached_preview_key,
            commands::camera_thumbs,
            commands::list_presets,
            commands::save_preset,
            commands::delete_preset,
            commands::export_presets,
            commands::read_text_file,
            commands::read_sidecar,
            commands::write_sidecar,
            commands::adobe_color_available,
            commands::read_lightroom_preset,
            commands::read_lightroom_edit,
            commands::parametric_curve_points,
            commands::auto_transform,
            commands::set_rating,
            commands::set_pick,
            commands::set_rating_and_pick,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_, event| {
            // Quitting: the Remove models leave the GPU with RapidOne.
            if let tauri::RunEvent::Exit = event {
                klein::shutdown();
            }
        });
}
