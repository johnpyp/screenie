# wlinput

A dev tool that drives a Wayland session with virtual pointer and keyboard
(`zwlr_virtual_pointer_v1`, `zwp_virtual_keyboard_v1`) for testing the UI headlessly.

```sh
wlinput move 300 200 , sleep 50 , down , move 700 520 , sleep 1500 , up
wlinput drag 100 100 800 600 15
```

Commands are separated by ` , `. Exiting releases held buttons, so add a trailing
`sleep` when you want a screenshot mid-gesture.
