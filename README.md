# Nimbus

A small island at the top centre of your screen with your computer's everyday controls one click away, and an AI assistant that can do small chores on your desktop for you.

Collapsed, it shows what matters right now: the music playing, the time. Opened, it gives you:

- Media, volume, brightness, Wi-Fi and Bluetooth, dark mode
- Wallpapers (Wallpaper Engine included)
- System stats, an app launcher, file search, clipboard history
- A day view with to-dos and a timer, and power actions

The island works fully without AI. When you want the assistant, it runs on the model you choose:

- **Local:** Nimbus can download and run a model on your own computer (llama.cpp). It loads the model when you ask something and unloads it after 30 minutes without use.
- **Cloud:** any OpenAI-compatible API. With a cloud model set, nothing is loaded locally.

The assistant can search the web, read pages, play music on Spotify or YouTube, read and write mail, and, if you allow it in Settings, see the screen and click or type in apps. Anything risky (sending, deleting, buying) asks you first. Voice: "Hey Nimbus" and push to talk, with speech to text from a local Whisper model.

Plugins add tools through the Model Context Protocol (programs over standard input and output) and can reach Nimbus through a local API that only programs on your computer can use.

## Download

Get the installer for your system from [Releases](../../releases/latest):

- **Windows:** `.msi` or `-setup.exe`
- **macOS:** `.dmg` (Apple silicon and Intel)
- **Linux:** `.AppImage`, `.deb` or `.rpm`

The installers are not code-signed yet:
- **Windows** may show a SmartScreen warning: choose "More info", then "Run anyway".
- **macOS:** the first time, right-click the app and choose Open.

Nimbus is built and used daily on Windows. The macOS and Linux builds are new and less tested, and some system controls are Windows only for now.

## Build it yourself

You need Rust (1.85 or newer), Node.js 22, and on Linux the [Tauri system libraries](https://v2.tauri.app/start/prerequisites/).

```sh
cd app
npm ci
npm run tauri dev      # run it
npm run tauri build    # make an installer
```

## Privacy

Nimbus keeps everything on your computer:
- Clipboard history, the file index and mail sign-ins stay local. Sign-ins are kept in the system's credential store.
- The screen is only looked at while you have allowed it and Nimbus is working on a request; frames are kept in memory, never on disk.
- Nothing is sent anywhere except to the AI model and the services you set up yourself.

## License

MIT
