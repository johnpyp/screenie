// Screen casts that don't show in the top bar. Screenie takes stills as the first frame
// of a short cast of each monitor (Mutter's are the fast way to the pixels), and the
// top bar would show the cast, in the very picture. So it says when the cast it starts
// is one (`arm`); Mutter makes the cast's handle while starting it, and the top bar's
// indicators are kept from seeing that handle.

import GLib from 'gi://GLib';

import {
    RemoteAccessApplet, ScreenSharingIndicator,
} from 'resource:///org/gnome/shell/ui/status/remoteAccess.js';

/** How long an unclaimed `arm` lasts. */
const ARMED_US = 2 * GLib.USEC_PER_SEC;

export class QuietCasts {
    constructor() {
        /** Until when the next recording handle is ours (monotonic µs), or 0. */
        this._armedUntil = 0;
        /** Handles seen, and those that are ours. */
        this._seen = new Set();
        this._ours = new Set();
        this._patched = [];
        for (const indicator of [RemoteAccessApplet, ScreenSharingIndicator])
            this._patch(indicator.prototype);
    }

    /** Whether the top bar's indicators could be kept from seeing casts. */
    get works() {
        return this._patched.length === 2;
    }

    /** The next cast of the screen to start is screenie's, and quiet. */
    arm() {
        this._armedUntil = GLib.get_monotonic_time() + ARMED_US;
    }

    /** It started, or didn't. */
    disarm() {
        this._armedUntil = 0;
    }

    destroy() {
        for (const [proto, original] of this._patched)
            proto._onNewHandle = original;
        this._patched = [];
    }

    _patch(proto) {
        const original = proto._onNewHandle;
        if (typeof original !== 'function')
            return;
        const casts = this;
        proto._onNewHandle = function (handle) {
            if (!casts._isOurs(handle))
                original.call(this, handle);
        };
        this._patched.push([proto, original]);
    }

    _isOurs(handle) {
        if (!this._seen.has(handle)) {
            this._seen.add(handle);
            // A still's cast is marked a recording; one per arm.
            if (handle.is_recording && GLib.get_monotonic_time() < this._armedUntil) {
                this._armedUntil = 0;
                this._ours.add(handle);
            }
            handle.connect('stopped', () => {
                this._seen.delete(handle);
                this._ours.delete(handle);
            });
        }
        return this._ours.has(handle);
    }
}
