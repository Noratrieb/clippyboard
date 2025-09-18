# clippyboard

clippyboard is a Wayland clipboard manager daemon and UI.

It provides a daemon that stores a clipboard history in memory and provides a socket to read and manage it.
A client program can then connect to it and read the contents and choose an item to copy to the clipboard again.

A barebones egui-based client is provided for doing this.

clippyboard provides first-class support for images!

clippyboard currently supports the following MIME types:
- `text/plain`
- `image/png`
- `image/jpg`

It will try to read out one of them (in descending preference) and store that value and provide it later.
If no supported MIME type is found, the clipboard entry is not stored.

https://github.com/user-attachments/assets/0bfdfe39-1177-4d11-bf5a-63e738751d7a
