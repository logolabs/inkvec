# Troubleshooting and questions

## Installing and starting

**Windows says "Windows protected your PC".** The installer is not code-signed. Choose **More
info**, then **Run anyway**. Check the file's SHA-256 against the release notes first if you want
to be sure it is the published one. See [Install](install.md#opening-the-app-the-first-time).

**macOS says the app cannot be opened.** Also unsigned. Control-click the app in Applications,
choose **Open**, then **Open** again. If that is refused too, run
`xattr -dr com.apple.quarantine "/Applications/Inkvec Studio.app"`.

**The Linux package will not install or start.** The app needs glibc 2.38, libstdc++ 13 and
WebKitGTK 4.1 (Ubuntu 23.10, Debian 13, Fedora 39 or newer), because the denoiser's runtime needs
them. On an older system use the command-line archive, which runs on glibc 2.17 and later (without
the denoiser).

**The Windows app will not start.** It needs Microsoft WebView2, which the `.exe` installer
fetches if it is missing. After installing from the `.msi`, or on a machine where WebView2 was
removed, install the *WebView2 Runtime* from Microsoft and start the app again.

## Transparency and backgrounds

**My logo has a transparent background, and the SVG has a white (or black) box behind it.**
Check that **Trace transparency** (Tune, Colour) is on; it is by default. If the image itself has
a solid background, turn on **Transparent background** (Tune, Output): the shape that covers the
whole canvas is then not painted.

**The holes in letters (the middle of an O) are filled in.** Transparency was flattened: turn
**Trace transparency** on. If you must trace with it off, **Holes as cutouts** puts the holes back.

**A white logo on a transparent background disappeared.** It is there; a white drawing on a white
page is invisible. With **Trace transparency** on, it is traced as white shapes on a clear ground.
Look at it with the Minify tab's dark **Backdrop**, or in the viewer (the stage is dark).

**Soft shadows or glows came back as flat shapes.** With **Trace transparency** on they come back as
translucent fills. Where a shade is a continuous blur, the Result tab says that part is a ramp SVG
cannot hold exactly.

## Colours

**Two shades of what should be one colour.** A flat colour measured twice, usually from
anti-aliasing or compression. Accept the [suggested colour groups](palette.md#suggested-groups), or
raise **Colour merging** a little.

**More colours than the design has.** Set **Colours** in the palette to the number the design has
and accept the suggestions, or lower **Max colours**.

**A small detail took the colour of its neighbour.** Too few colours, or merging too strong. Raise
**Max colours** or lower **Colour merging**, and undo any group that swallowed it.

**Does the transparent background use up one of the colours?** No. With **Trace transparency** on
(the default), the clear ground is not counted towards **Max colours**.

**The colours are slightly off from the brand's.** The colours written are the ones measured in
the image, and a JPEG or a screenshot changes them. Use the [denoiser](settings.md#denoiser), and
[paste the brand palette](palette.md#paste-a-brand-palette) to snap each ink to its real value.

**A snapped colour went back to the measured one.** A snap is dropped when its ink is no longer in
the drawing, for example after a colour group merged it into another ink. Snap the merged ink, or
give the group a **Custom…** colour.

## The denoiser

**The download fails.** The app downloads from `huggingface.co`. A proxy or firewall that blocks it
stops the download; the message says why. You can also fetch the model yourself, from
`https://huggingface.co/Logolabs/inkvec-denoiser-001/resolve/main/restorer.onnx`, and put it where
the app keeps it (see [Settings](settings.md#where-things-are-kept)). The app checks it against the
published SHA-256 and only treats it as installed if it matches.

**The download finished but the denoiser says "Not installed".** The file did not match the
published checksum (an interrupted or altered download) and was not kept. Download it again.

**"Not in this build".** The Intel macOS build has no denoiser, because its runtime has no package
for that platform.

**The denoiser is on and the draft looks no different.** Drafts skip the denoiser. It runs on the
full trace, a moment after you stop moving controls.

## Speed and size

**A trace is slow.** Tracing time grows with the pixel count. Lower **Trace size** (Tune, Detail);
for most logos 1024 or 2048 px loses nothing visible. **Threads** in Settings should be the full
count unless you need the computer for something else.

**"The trace would run out of memory".** Press **Retry at** with the size it suggests.

**The SVG is huge.** Try the *Fewer paths* preset, raise **Precision**, accept the colour-group
suggestions, and export the minified copy. A photograph will never make a small SVG; see
[Limitations](../LIMITATIONS.md).

**The trace of an enlarged image is blocky or stepped.** An image that was enlarged from a smaller
one carries the smaller one's pixel steps, and the trace follows them faithfully. Trace the
original, smaller file instead.

## The result

**The lettering is outlines, not text.** Tracing recovers shapes, not the typeface; the Result tab
says so. Re-set the words in a vector editor if they must stay editable.

**Lines came back as thin filled shapes.** Try the *Line art* preset. It only works for lines of one
even width; brush and calligraphic strokes stay as outlines.

**The draft and the final look different.** A draft is traced small (512 px) for speed. Wait for
the chip to say **final** before judging or copying.

**The favicon has space above and below the logo.** Favicons are square; a wide drawing is
centred in them on a transparent background rather than stretched. Trace a square crop of the mark
for a favicon that fills its square.

## Integrations

**"Vectorize with Inkvec" is not in the right-click menu.** Add it first in
[Settings, Advanced](settings.md#advanced). On Windows 11 it is under **Show more options**. On
macOS use **Open With**, where the app is already listed.

**`inkvec` is not found after "Add inkvec to PATH".** Open a new terminal. On macOS, add
`~/.local/bin` to your `PATH`.

**A cutter imports the file at the wrong size.** In Fabricate, set **The files state their size
in** to the pixel rate the program uses (Cricut and Silhouette: 72/in), and cut the size-check
square to confirm. See [Fabricate](fabricate.md#size).

## Help and settings

**Help says the guide is not bundled with this build.** A development or browser build may not
include it. **Open the online guide** shows the same pages on
[logolabs.github.io/inkvec](https://logolabs.github.io/inkvec/studio/).

**How do I start over?** Settings, Advanced, **Reset settings**. It resets your preferences and
clears the recent files, and keeps your saved presets, your files and the denoiser.

## Studio Lite

**Where did my file go?** Studio Lite downloads what it saves, into your browser's downloads
folder.

**Where are Batch, the command line and the right-click menu?** They need the desktop app. See
[Inkvec Studio and Inkvec Studio Lite](README.md#inkvec-studio-and-inkvec-studio-lite).

**The first visit takes a few seconds to start.** The loading screen is the engine arriving
(about 5 MB) and starting; its bar follows the real download. Later visits start from the
browser's cache.

**The denoiser is on but the trace looks the same, and the rail says it is downloading.** The
trace shown was made without it while it downloads (about 100 MB, once). It traces again by
itself when the denoiser is ready. If it failed, press **Retry** under the Denoiser buttons or in
the status strip.

**Nothing is downloaded in the background.** Your browser asks sites to save data, so Studio
Lite waits until you turn the denoiser on. Inside the Hugging Face frame the denoiser cannot run
at all: use **Open in its own tab**.

**My settings are not remembered.** A private window, or a browser set to block site data, gives
the page nowhere to keep them. The app still works, from the defaults each time.

## Reporting a problem

Report bugs on [GitHub](https://github.com/logolabs/inkvec/issues). Include the app version (from
About), your operating system, and, if you can share it, the image. Security problems go through
the [security policy](../../SECURITY.md) instead.
