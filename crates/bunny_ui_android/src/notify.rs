//! The phone's notifications — `NotificationManager` over JNI — answering
//! the notification doors of `bunny_ui::app`: `notify` posts one, and a
//! tap on it comes back as `AppEvent::NotificationActivated`.
//!
//! A `NativeActivity` hears no new intent: `onNewIntent` is Java, and
//! this shell has none. So the tap does not deliver an intent to the
//! running activity — it RE-CREATES the activity with the intent
//! (`CLEAR_TOP` without `SINGLE_TOP`, on the standard launch mode), and
//! the shell reads the intent's extras at create, once the app's own
//! `main` ran and subscribed. The process lives through it; the scene
//! is rebuilt from the app's own state, as after any re-creation.
//!
//! The permission is the platform's from API 33: the first post asks
//! the person and refuses by name, and the next post reads the answer
//! — a `NativeActivity` never hears `onRequestPermissionsResult`.

use std::cell::Cell;
use std::ptr::null_mut;

use bunny_ui::app::Notification;

use crate::jni::{Env, Frame, JObject, boolean, int, object};

/// The one channel every notification of the app rides.
const CHANNEL: &str = "bunny";
/// The intent extras that say which notification, and which button.
const ID_EXTRA: &str = "bunny.notification.id";
const ACTION_EXTRA: &str = "bunny.notification.action";
/// `Intent.FLAG_ACTIVITY_NEW_TASK | FLAG_ACTIVITY_CLEAR_TOP`: the
/// running activity is finished and re-created with this intent.
const RECREATE: i32 = 0x1000_0000 | 0x0400_0000;
/// `Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY`: the task was brought
/// back from the recents, with the intent it was last given.
const FROM_HISTORY: i32 = 0x0010_0000;
/// `PendingIntent.FLAG_IMMUTABLE | FLAG_UPDATE_CURRENT`.
const PENDING_FLAGS: i32 = 0x0400_0000 | 0x0800_0000;
/// `NotificationManager.IMPORTANCE_HIGH`: a banner over what is shown.
const IMPORTANCE_HIGH: i32 = 4;
const POST_NOTIFICATIONS: &str = "android.permission.POST_NOTIFICATIONS";

thread_local! {
    static CHANNEL_MADE: Cell<bool> = const { Cell::new(false) };
    /// The person was asked once this process; the answer is read
    /// from the manager at the next post.
    static PERMISSION_ASKED: Cell<bool> = const { Cell::new(false) };
}

/// Shows a notification, or says why it cannot: no manager, a person
/// who has not allowed it yet, a person who said no. The same `id`
/// REPLACES — the manager keeps one per tag.
pub fn notify(notification: &Notification) -> Result<(), String> {
    let Some(env) = Env::current() else {
        return Err(String::from("no JNI env on this thread: notify from the app's own thread"));
    };
    let Some(_frame) = Frame::new(env, 64) else {
        return Err(String::from("the JNI local frame was refused"));
    };
    let manager = manager(env).ok_or("the notification service is not available")?;
    if !enabled(env, manager).unwrap_or(false) {
        if crate::ffi::sdk_version() >= 33 && !PERMISSION_ASKED.with(|slot| slot.replace(true)) {
            ask_permission(env);
            return Err(String::from(
                "asked the person for permission to notify; post again after they answer",
            ));
        }
        return Err(String::from("the person did not allow notifications for this app"));
    }
    if !CHANNEL_MADE.with(Cell::get) {
        make_channel(env, manager).ok_or("the notification channel could not be made")?;
        CHANNEL_MADE.with(|slot| slot.set(true));
    }
    let built = build(env, notification)?;
    post(env, manager, &notification.id, built).ok_or("the post was refused")?;
    Ok(())
}

/// The notification that created this activity, if one did: its id
/// and the button pressed (`None` for the body). Read ONCE — the
/// extras are taken off the intent the activity keeps — and never
/// from a task the recents brought back. The notification is cleared
/// from the shade: a button's tap leaves it, the body's takes it.
pub fn launch_activation() -> Option<(String, Option<String>)> {
    let env = Env::current()?;
    let _frame = Frame::new(env, 16)?;
    let activity = env.activity();
    let activity_class = env.class(c"android/app/Activity")?;
    let get_intent = env.method(activity_class, c"getIntent", c"()Landroid/content/Intent;")?;
    let intent = env.call_object(activity, get_intent, &[])?;
    let intent_class = env.class(c"android/content/Intent")?;
    let get_flags = env.method(intent_class, c"getFlags", c"()I")?;
    if env.call_int(intent, get_flags, &[])? & FROM_HISTORY != 0 {
        return None;
    }
    let get_extra =
        env.method(intent_class, c"getStringExtra", c"(Ljava/lang/String;)Ljava/lang/String;")?;
    let remove_extra = env.method(intent_class, c"removeExtra", c"(Ljava/lang/String;)V")?;
    let take = |key: &str| -> Option<String> {
        let name = env.string(key)?;
        let value = env.call_object(intent, get_extra, &[object(name)]);
        env.call_void(intent, remove_extra, &[object(name)]);
        env.to_string(value?)
    };
    let id = take(ID_EXTRA)?;
    let action = take(ACTION_EXTRA);
    if let Some(manager) = manager(env) {
        cancel(env, manager, &id);
    }
    Some((id, action))
}

/// `Context.getSystemService("notification")`.
fn manager(env: Env) -> Option<JObject> {
    let context_class = env.class(c"android/content/Context")?;
    let get_service =
        env.method(context_class, c"getSystemService", c"(Ljava/lang/String;)Ljava/lang/Object;")?;
    let name = env.string("notification")?;
    env.call_object(env.activity(), get_service, &[object(name)])
}

fn enabled(env: Env, manager: JObject) -> Option<bool> {
    let manager_class = env.class(c"android/app/NotificationManager")?;
    let are_enabled = env.method(manager_class, c"areNotificationsEnabled", c"()Z")?;
    env.call_bool(manager, are_enabled, &[])
}

/// `Activity.requestPermissions([POST_NOTIFICATIONS], 1)` — the dialog
/// shows; the answer is never delivered here, and is read from the
/// manager at the next post.
fn ask_permission(env: Env) -> Option<()> {
    let string_class = env.class(c"java/lang/String")?;
    let permission = env.string(POST_NOTIFICATIONS)?;
    let wanted = env.new_object_array(string_class, 1, permission)?;
    let activity_class = env.class(c"android/app/Activity")?;
    let request = env.method(activity_class, c"requestPermissions", c"([Ljava/lang/String;I)V")?;
    env.call_void(env.activity(), request, &[object(wanted), int(1)]).then_some(())
}

/// The channel, made once: the platform keeps it, and a second
/// creation is a no-op there too.
fn make_channel(env: Env, manager: JObject) -> Option<()> {
    let channel_class = env.class(c"android/app/NotificationChannel")?;
    let new_channel = env.method(
        channel_class,
        c"<init>",
        c"(Ljava/lang/String;Ljava/lang/CharSequence;I)V",
    )?;
    let channel = env.new_object(
        channel_class,
        new_channel,
        &[object(env.string(CHANNEL)?), object(env.string("Notifications")?), int(IMPORTANCE_HIGH)],
    )?;
    let manager_class = env.class(c"android/app/NotificationManager")?;
    let create = env.method(
        manager_class,
        c"createNotificationChannel",
        c"(Landroid/app/NotificationChannel;)V",
    )?;
    env.call_void(manager, create, &[object(channel)]).then_some(())
}

/// The `Notification`, built: the texts, the small icon (the app's
/// own, or the platform's when the app declares none — a notification
/// without one is refused), the tap, and a button per action.
fn build(env: Env, notification: &Notification) -> Result<JObject, String> {
    let activity = env.activity();
    let builder_class =
        env.class(c"android/app/Notification$Builder").ok_or("no Notification.Builder class")?;
    let new_builder = env
        .method(builder_class, c"<init>", c"(Landroid/content/Context;Ljava/lang/String;)V")
        .ok_or("no Notification.Builder constructor")?;
    let channel = env.string(CHANNEL).ok_or("the channel's name")?;
    let builder = env
        .new_object(builder_class, new_builder, &[object(activity), object(channel)])
        .ok_or("the builder could not be made")?;
    let set = |name: &std::ffi::CStr, signature: &std::ffi::CStr, args: &[crate::jni::JValue]| {
        let method = env.method(builder_class, name, signature)?;
        env.call_object(builder, method, args)
    };
    let title = env.string(&notification.title).ok_or("the title")?;
    set(c"setContentTitle", c"(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;", &[
        object(title),
    ])
    .ok_or("the title was refused")?;
    let body = env.string(&notification.body).ok_or("the body")?;
    set(c"setContentText", c"(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;", &[
        object(body),
    ])
    .ok_or("the body was refused")?;
    let icon = small_icon(env).ok_or("no icon for the notification")?;
    set(c"setSmallIcon", c"(I)Landroid/app/Notification$Builder;", &[int(icon)])
        .ok_or("the icon was refused")?;
    set(c"setAutoCancel", c"(Z)Landroid/app/Notification$Builder;", &[boolean(true)])
        .ok_or("auto-cancel was refused")?;
    let tap = pending(env, &notification.id, None).ok_or("the tap's intent could not be made")?;
    set(c"setContentIntent", c"(Landroid/app/PendingIntent;)Landroid/app/Notification$Builder;", &[
        object(tap),
    ])
    .ok_or("the tap was refused")?;
    for action in &notification.actions {
        let button = button(env, &notification.id, &action.key, &action.label)
            .ok_or_else(|| format!("the button `{}` could not be made", action.key))?;
        set(c"addAction", c"(Landroid/app/Notification$Action;)Landroid/app/Notification$Builder;", &[
            object(button),
        ])
        .ok_or_else(|| format!("the button `{}` was refused", action.key))?;
    }
    let finish = env
        .method(builder_class, c"build", c"()Landroid/app/Notification;")
        .ok_or("no Notification.Builder.build")?;
    env.call_object(builder, finish, &[]).ok_or_else(|| String::from("the build was refused"))
}

/// The app's own icon resource, or the platform's dialog icon when
/// the manifest declares none.
fn small_icon(env: Env) -> Option<i32> {
    let context_class = env.class(c"android/content/Context")?;
    let get_info = env.method(
        context_class,
        c"getApplicationInfo",
        c"()Landroid/content/pm/ApplicationInfo;",
    )?;
    let info = env.call_object(env.activity(), get_info, &[])?;
    let info_class = env.class(c"android/content/pm/ApplicationInfo")?;
    let icon = env.int_field(info, env.field(info_class, c"icon", c"I")?)?;
    if icon != 0 {
        return Some(icon);
    }
    let drawable = env.class(c"android/R$drawable")?;
    env.static_int_field(drawable, env.static_field(drawable, c"ic_dialog_info", c"I")?)
}

/// One button: its label, and the intent that names its key.
fn button(env: Env, id: &str, key: &str, label: &str) -> Option<JObject> {
    let intent = pending(env, id, Some(key))?;
    let builder_class = env.class(c"android/app/Notification$Action$Builder")?;
    let new_builder = env.method(
        builder_class,
        c"<init>",
        c"(Landroid/graphics/drawable/Icon;Ljava/lang/CharSequence;Landroid/app/PendingIntent;)V",
    )?;
    let label = env.string(label)?;
    // the icon may be null: a button is its label
    let builder = env.new_object(
        builder_class,
        new_builder,
        &[object(null_mut()), object(label), object(intent)],
    )?;
    let finish = env.method(builder_class, c"build", c"()Landroid/app/Notification$Action;")?;
    env.call_object(builder, finish, &[])
}

/// The intent a tap fires: THIS activity, re-created, carrying the
/// notification's id and the button's key. A pending intent is one
/// per (request code, intent) and the platform compares intents
/// WITHOUT their extras — so each (id, key) also gets its own data
/// url and its own code, or a button would inherit the body's extras.
fn pending(env: Env, id: &str, action: Option<&str>) -> Option<JObject> {
    let activity = env.activity();
    let intent_class = env.class(c"android/content/Intent")?;
    let intent = env.new_object(intent_class, env.method(intent_class, c"<init>", c"()V")?, &[])?;
    let activity_class = env.class(c"android/app/Activity")?;
    let get_component =
        env.method(activity_class, c"getComponentName", c"()Landroid/content/ComponentName;")?;
    let component = env.call_object(activity, get_component, &[])?;
    let set_component = env.method(
        intent_class,
        c"setComponent",
        c"(Landroid/content/ComponentName;)Landroid/content/Intent;",
    )?;
    env.call_object(intent, set_component, &[object(component)])?;
    let set_flags = env.method(intent_class, c"setFlags", c"(I)Landroid/content/Intent;")?;
    env.call_object(intent, set_flags, &[int(RECREATE)])?;
    let put_extra = env.method(
        intent_class,
        c"putExtra",
        c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
    )?;
    env.call_object(intent, put_extra, &[object(env.string(ID_EXTRA)?), object(env.string(id)?)])?;
    if let Some(key) = action {
        env.call_object(intent, put_extra, &[
            object(env.string(ACTION_EXTRA)?),
            object(env.string(key)?),
        ])?;
    }
    let mut named = Vec::with_capacity(id.len() + 1 + action.map_or(0, str::len));
    named.extend_from_slice(id.as_bytes());
    named.push(0);
    named.extend_from_slice(action.unwrap_or("").as_bytes());
    let code = crate::face::fnv64(&named) as i32;
    let uri_class = env.class(c"android/net/Uri")?;
    let parse = env.static_method(uri_class, c"parse", c"(Ljava/lang/String;)Landroid/net/Uri;")?;
    let url = env.string(&format!("bunny://notification/{:08x}", code as u32))?;
    let uri = env.call_static_object(uri_class, parse, &[object(url)])?;
    let set_data = env.method(intent_class, c"setData", c"(Landroid/net/Uri;)Landroid/content/Intent;")?;
    env.call_object(intent, set_data, &[object(uri)])?;
    let pending_class = env.class(c"android/app/PendingIntent")?;
    let get_activity = env.static_method(
        pending_class,
        c"getActivity",
        c"(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;",
    )?;
    env.call_static_object(pending_class, get_activity, &[
        object(activity),
        int(code),
        object(intent),
        int(PENDING_FLAGS),
    ])
}

/// `NotificationManager.notify(tag, 0, notification)` — the tag is the
/// app's id, and the same tag replaces.
fn post(env: Env, manager: JObject, id: &str, notification: JObject) -> Option<()> {
    let manager_class = env.class(c"android/app/NotificationManager")?;
    let notify = env.method(
        manager_class,
        c"notify",
        c"(Ljava/lang/String;ILandroid/app/Notification;)V",
    )?;
    let tag = env.string(id)?;
    env.call_void(manager, notify, &[object(tag), int(0), object(notification)]).then_some(())
}

fn cancel(env: Env, manager: JObject, id: &str) -> Option<()> {
    let manager_class = env.class(c"android/app/NotificationManager")?;
    let cancel = env.method(manager_class, c"cancel", c"(Ljava/lang/String;I)V")?;
    let tag = env.string(id)?;
    env.call_void(manager, cancel, &[object(tag), int(0)]).then_some(())
}
