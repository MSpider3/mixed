# The Beginner's Guide to Installing `mixed` on Android (via Termux)

Welcome! If you've never used a command-line terminal on your phone before, don't worry. This guide assumes zero prior experience and will walk you through every exact step needed to get the `mixed` TUI (Terminal User Interface) music player running beautifully on your Android device.

---

## Step 1: Install Termux and Termux:API
To run Linux terminal applications on Android, you need an app called **Termux**. Because we also want the music player to show a media notification in your Android status bar, we need a companion app called **Termux:API**.

1. Download **Termux** and **Termux:API** from one of these stores:
   * **F-Droid** (Highly Recommended)
   * **Aurora Store**
   * **Google Play Store** *(Note: The Play Store version is no longer officially updated, so F-Droid or Aurora is preferred).*
2. **Important:** Both apps must be downloaded from the *same store* so their security signatures match.

## Step 2: Grant Storage Permissions
Termux is a secure sandbox. By default, it cannot see your phone's internal storage (where your music and downloads live). We need to give it the keys.

1. Open the **Termux** app.
2. Type the following command and press Enter:
   ```bash
   termux-setup-storage
   ```
3. A standard Android popup will appear asking for permission to access files. Tap **Allow**.

## Step 3: Update and Install Required Libraries
Before installing `mixed`, we need to make sure Termux has the latest system updates and the background audio engine (`mpv`).

1. Update the core system by typing this and pressing Enter:
   ```bash
   pkg update && pkg upgrade -y
   ```
   *(If it pauses and asks you a question with a [Y/n] prompt, just type `Y` and press Enter).*
2. Install the required audio and API libraries:
   ```bash
   pkg install mpv termux-api ffmpeg -y
   ```

## Step 4: Download the `mixed` Player
1. Open your phone's web browser and go to the official GitHub releases page: `https://github.com/MehulGolecha/mixed/releases`
2. Download the Android-specific file. It will look something like this: `mixed-v0.1.2-aarch64-linux-android.tar.gz`.
3. **We will assume this file has been saved to your standard Android `Downloads` folder.**

## Step 5: Extract and Install the App
Now we will grab the downloaded file, extract it, and install it globally in Termux so you can run it from anywhere.

1. Navigate to your phone's Downloads folder inside Termux:
   ```bash
   cd /storage/emulated/0/Download
   ```
2. Extract the downloaded file (replace the filename if the version number is different):
   ```bash
   tar -xzf mixed-*-aarch64-linux-android.tar.gz
   ```
   *(Alternatively, you can manually extract the `.tar.gz` file using your phone's built-in File Manager app).*
3. **CRITICAL STEP:** Android does not allow you to run programs directly from the Downloads folder for security reasons. We must copy the extracted `mixed` file back to Termux's private home folder:
   ```bash
   cp mixed ~
   ```
4. Go back to your Termux home folder:
   ```bash
   cd ~
   ```
5. Grant the file permission to execute:
   ```bash
   chmod +x mixed
   ```
6. Move the app into your global system path so it's permanently installed:
   ```bash
   mv mixed $PREFIX/bin/
   ```

## Step 6: Play Your Music!
The installation is complete! To run the application:

1. **Rotate your phone into Landscape mode.** (The app requires a wide screen to display the interface correctly).
2. Type `mixed` followed by the location of your music folder. For most Android phones, the default music folder is located here:
   ```bash
   mixed "/storage/emulated/0/Music"
   ```
3. Press Enter, and enjoy the music!

---

## Troubleshooting & Common Problems

**Issue: My keyboard isn't working or isn't typing anything in Termux!**
* **Fix:** Some custom Android keyboards do not send terminal keystrokes properly. Switch your phone's default keyboard or **Gboard** (most phones default keyboard) while using Termux.

**Issue: I get a `Permission denied` error when running `chmod +x mixed`.**
* **Fix:** You are trying to make the file executable while it is still sitting in the `/storage/emulated/0/Download` folder. You must copy it to the Termux home folder (`cp mixed ~`) and navigate there (`cd ~`) *before* running the `chmod` command.

**Issue: I get a `Permission denied` error when trying to access the Download folder.**
* **Fix:** Termux doesn't have storage access. Run `termux-setup-storage` and accept the Android prompt.

**Issue: `CANNOT LINK EXECUTABLE` or `dpkg` errors when installing `mpv` or `ffmpeg`.**
* **Fix:** Your Termux system libraries are out of sync. Force an update by running:
  ```bash
  apt --fix-broken install
  pkg update && pkg upgrade -y
  ```
  Then try installing `mpv` again.

**Issue: The music plays, but there is no Android notification bar widget.**
* **Fix:** Ensure you actually installed the `Termux:API` app from the App Store (Step 1), and ensure you ran `pkg install termux-api` in the terminal (Step 3). Both the app and the terminal package must be present.
