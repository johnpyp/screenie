# wlinput

A dev tool that drives a Wayland session with virtual pointer and keyboard
(`zwlr_virtual_pointer_v1`, `zwp_virtual_keyboard_v1`) for testing the UI headlessly. On
KWin it uses `org_kde_kwin_fake_input` instead, which KWin offers only to a binary whose
desktop entry asks for it: `tools/desktop.sh input kde` installs one.

```sh
wlinput move 300 200 , sleep 50 , down , move 700 520 , sleep 1500 , up
wlinput drag 100 100 800 600 15
```

Commands are separated by ` , `. Exiting releases held buttons, so add a trailing
`sleep` when you want a screenshot mid-gesture.

`wlinput -` keeps one pointer and keyboard for a whole session (the e2e harness uses
it): it prints `ready`, then runs each stdin line as a command chain and answers `ok`
(or `error: …`) once the compositor has it. `type TEXT` types ASCII text.
