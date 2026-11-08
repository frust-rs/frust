# frust-photo-library

Pick photos and videos in frust apps with `PhotoLibrary::pick(&PickOptions)`: the system photo
picker on Android and iOS, a native file dialog on Linux, macOS and Windows.

Like every frust **platform plugin**, this crate is added to your app's own `Cargo.toml`
alongside `frust` — the `frust` facade does not re-export it.

More about Frust: <https://frust.dev> and <https://github.com/frust-rs/frust>.

## Platform support

| Platform | Backend |
|---|---|
| Android | system photo picker, picks copied into the app cache |
| iOS | `PHPickerViewController`, picks copied into the app cache |
| Linux, macOS, Windows | native file dialog (`frust-file-dialogs`), original files returned |
| web | none: `PhotoError::Unsupported` |

## Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-photo-library = "0.6"
```

## Use

```rust
use frust::prelude::*;
use frust_photo_library::{MediaFilter, PhotoLibrary, PickOptions};

struct Gallery;

impl Component for Gallery {
    fn build(&self, cx: &mut Cx) -> impl View {
        let pick = use_task(cx, || {
            // `pick` blocks until the user chooses: it runs off the UI thread here.
            PhotoLibrary::pick(&PickOptions::new().filter(MediaFilter::Images).limit(1))
        });

        column![
            button("Choose photo").on_tap(move |_| pick.run()),
            // Decode a picked `PickedItem.path` with `frust::decode_image_async`.
        ]
    }
}
```

An empty result means the user cancelled. On mobile each pick is a copy
(`PickedItem::is_copy`); call `PhotoLibrary::clear_pick_cache()` to reclaim the space.
