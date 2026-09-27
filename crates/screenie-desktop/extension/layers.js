// Screenie's windows as the layer-shell surfaces they are elsewhere. GNOME has no
// layer-shell, so screenie opens ordinary windows, having first described each (by its
// title) as the surface it stands for: which output, which edges it's anchored to, and
// whether it takes the keyboard.
//
// - One that takes the keyboard (the selector, the editor over the screen) is a
//   fullscreen window: it's kept above other windows, opens with no animation and with
//   the focus, and while it has it, GNOME's shortcuts and notification banners wait.
// - One that doesn't (preview cards, the recording's controls) is a dock: never focused,
//   on every workspace, placed where its anchors put it, and above other windows (docks
//   are, but not over a fullscreen one: made "always on top" as well, they'd keep new
//   windows they cover from being focused). Only from GNOME 49, where any window can be
//   made one.
// Neither is in alt-tab, the overview or the dash (from GNOME 49).

import GLib from 'gi://GLib';
import Meta from 'gi://Meta';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

/** How long a description waits for its window. */
const EXPECT_US = 5 * GLib.USEC_PER_SEC;

// Anchors, as layer-shell has them.
const TOP = 1;
const BOTTOM = 2;
const LEFT = 4;
const RIGHT = 8;

export class Layers {
    constructor() {
        /** title → {pid, layer, until} */
        this._expected = new Map();
        /** MetaWindow → its layer */
        this._windows = new Map();
        this._blockedBanners = false;

        this._signals = [
            [global.display, global.display.connect('window-created',
                (display, window) => this._created(window))],
            [global.window_manager, global.window_manager.connect('map',
                (wm, actor) => this._mapped(actor.meta_window))],
            [global.window_manager, global.window_manager.connect('filter-keybinding',
                () => this._hasKeyboard())],
            [global.display, global.display.connect('notify::focus-window',
                () => this._syncBanners())],
        ];

        const layers = this;
        this._shouldAnimate = Main.wm._shouldAnimateActor;
        Main.wm._shouldAnimateActor = function (actor, types) {
            if (layers._windows.has(actor.meta_window))
                return false;
            return layers._shouldAnimate.call(this, actor, types);
        };
    }

    /** Whether windows can be docks (floating surfaces). */
    get floats() {
        return typeof Meta.Window.prototype.set_type === 'function';
    }

    destroy() {
        Main.wm._shouldAnimateActor = this._shouldAnimate;
        for (const [object, id] of this._signals)
            object.disconnect(id);
        this._signals = [];
        if (this._blockedBanners)
            Main.messageTray.bannerBlocked = false;
        this._windows.clear();
        this._expected.clear();
    }

    /**
     * Process `pid`'s next window titled `title` is the surface `layer` describes.
     *
     * @param {number} pid - screenie's process
     * @param {string} title - the window's title
     * @param {object} layer - output (connector), anchor (edges), margin
     *   ([top, right, bottom, left]), keyboard (takes it), exclusive (-1: ignores the top
     *   bar)
     */
    expect(pid, title, layer) {
        const now = GLib.get_monotonic_time();
        for (const [t, e] of this._expected) {
            if (e.until < now)
                this._expected.delete(t);
        }
        this._expected.set(title, {pid, layer, until: now + EXPECT_US});
    }

    _created(window) {
        const pid = window.get_pid();
        if (![...this._expected.values()].some(e => e.pid === pid))
            return;
        // The title comes a moment after the window, before its first frame.
        if (this._claim(window))
            return;
        const id = window.connect('notify::title', () => {
            if (this._claim(window))
                window.disconnect(id);
        });
        window.connect('unmanaged', () => window.disconnect(id));
    }

    /** Take `window` on as the surface it was described as, if it was. */
    _claim(window) {
        const title = window.get_title();
        const expected = this._expected.get(title);
        if (!expected || expected.pid !== window.get_pid())
            return false;
        this._expected.delete(title);
        const {layer} = expected;
        this._windows.set(window, layer);
        window.connect('unmanaged', () => {
            this._windows.delete(window);
            this._syncBanners();
        });
        window.hide_from_window_list?.();
        if (layer.keyboard) {
            window.make_above();
            // Focused once shown, with the time now: GNOME may otherwise hold it back
            // (behind a window kept above, say).
            const id = window.connect('shown', () => {
                window.disconnect(id);
                window.activate(global.display.get_current_time_roundtrip());
            });
        } else if (this.floats) {
            window.set_type(Meta.WindowType.DOCK);
            window.connect('size-changed', () => this._place(window, layer));
        }
        return true;
    }

    _mapped(window) {
        const layer = this._windows.get(window);
        if (layer && !layer.keyboard)
            this._place(window, layer);
    }

    /** Put a floating surface where its anchors and margins say. */
    _place(window, layer) {
        let monitor = global.backend.get_monitor_manager()
            .get_monitor_for_connector(layer.output);
        // No output named: the one the pointer is on, as layer-shell compositors choose.
        if (monitor < 0)
            monitor = global.display.get_current_monitor();
        const area = layer.exclusive < 0
            ? global.display.get_monitor_geometry(monitor)
            : Main.layoutManager.getWorkAreaForMonitor(monitor);
        const [top, right, bottom, left] = layer.margin;
        const frame = window.get_frame_rect();
        const span = (start, length, size, before, after, atStart, atEnd) => {
            if (atStart && atEnd)
                return [start + before, length - before - after];
            if (atStart)
                return [start + before, size];
            if (atEnd)
                return [start + length - size - after, size];
            return [start + Math.round((length - size) / 2), size];
        };
        const [x, width] = span(area.x, area.width, frame.width, left, right,
            layer.anchor & LEFT, layer.anchor & RIGHT);
        const [y, height] = span(area.y, area.height, frame.height, top, bottom,
            layer.anchor & TOP, layer.anchor & BOTTOM);
        if (width === frame.width && height === frame.height) {
            if (x !== frame.x || y !== frame.y)
                window.move_frame(true, x, y);
        } else {
            window.move_resize_frame(true, x, y, width, height);
        }
    }

    /** Whether one of screenie's surfaces that take the keyboard has it. */
    _hasKeyboard() {
        const focus = global.display.focus_window;
        return focus !== null && this._windows.get(focus)?.keyboard === true;
    }

    /** Notification banners wait while a surface that takes the keyboard has it. */
    _syncBanners() {
        const block = this._hasKeyboard();
        if (block === this._blockedBanners)
            return;
        this._blockedBanners = block;
        Main.messageTray.bannerBlocked = block;
    }
}
