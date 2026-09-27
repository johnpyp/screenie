// Screenie's GNOME Shell extension: what GNOME gives only to code in its shell, for
// screenie, over D-Bus (`dev.johnpyp.Screenie.Shell` on the session bus). See the
// screenie-desktop crate's README.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

import {screeniePid} from './callers.js';
import {QuietCasts} from './casts.js';
import {Layers} from './layers.js';
import * as Windows from './windows.js';

const NAME = 'dev.johnpyp.Screenie.Shell';
const PATH = '/dev/johnpyp/Screenie/Shell';
/** Goes up when the interface changes in a way screenie has to know. */
const VERSION = 1;

const INTERFACE = `
<node>
  <interface name="${NAME}">
    <property name="Version" type="u" access="read"/>
    <property name="Features" type="as" access="read"/>
    <method name="Windows">
      <arg name="windows" type="a(tss(iiii)b)" direction="out"/>
    </method>
    <method name="PointerOutput">
      <arg name="outputs" type="as" direction="in"/>
      <arg name="output" type="s" direction="out"/>
    </method>
    <method name="QuietNextCast"/>
    <method name="CastStarted"/>
    <method name="Place">
      <arg name="title" type="s" direction="in"/>
      <arg name="layer" type="a{sv}" direction="in"/>
    </method>
  </interface>
</node>`;

class Service {
    constructor() {
        this._casts = new QuietCasts();
        this._layers = new Layers();
    }

    destroy() {
        this._casts.destroy();
        this._layers.destroy();
    }

    get Version() {
        return VERSION;
    }

    get Features() {
        const features = ['windows', 'pointer', 'overlays'];
        if (this._casts.works)
            features.push('quiet-casts');
        if (this._layers.floats)
            features.push('floating');
        return features;
    }

    WindowsAsync(params, invocation) {
        this._serve(invocation, pid =>
            new GLib.Variant('(a(tss(iiii)b))', [Windows.shown(pid)]));
    }

    /** Which of `outputs` (connectors) the pointer is on, or none. */
    PointerOutputAsync([outputs], invocation) {
        this._serve(invocation, () => {
            const monitors = global.backend.get_monitor_manager();
            const current = global.display.get_current_monitor();
            const output = outputs.find(c => monitors.get_monitor_for_connector(c) === current);
            return new GLib.Variant('(s)', [output ?? '']);
        });
    }

    QuietNextCastAsync(params, invocation) {
        this._serve(invocation, () => this._casts.arm());
    }

    CastStartedAsync(params, invocation) {
        this._serve(invocation, () => this._casts.disarm());
    }

    PlaceAsync([title, layer], invocation) {
        this._serve(invocation, pid => {
            const {output, anchor, margin, keyboard, exclusive} = layer;
            this._layers.expect(pid, title, {
                output: output?.unpack() ?? '',
                anchor: anchor?.unpack() ?? 0,
                margin: margin?.deepUnpack() ?? [0, 0, 0, 0],
                keyboard: keyboard?.unpack() ?? false,
                exclusive: exclusive?.unpack() ?? 0,
            });
        });
    }

    /** Answer `invocation` with `answer(pid)` if the caller is screenie. */
    async _serve(invocation, answer) {
        try {
            const pid = await screeniePid(invocation.get_sender());
            if (pid === null) {
                invocation.return_error_literal(Gio.DBusError, Gio.DBusError.ACCESS_DENIED,
                    'Only screenie (the binary its desktop entry runs) may ask');
                return;
            }
            invocation.return_value(answer(pid) ?? null);
        } catch (e) {
            logError(e, 'screenie');
            invocation.return_error_literal(Gio.DBusError, Gio.DBusError.FAILED, `${e}`);
        }
    }
}

export default class ScreenieExtension extends Extension {
    enable() {
        this._service = new Service();
        this._object = Gio.DBusExportedObject.wrapJSObject(INTERFACE, this._service);
        this._object.export(Gio.DBus.session, PATH);
        this._name = Gio.bus_own_name_on_connection(Gio.DBus.session, NAME,
            Gio.BusNameOwnerFlags.NONE, null, null);
    }

    disable() {
        Gio.bus_unown_name(this._name);
        this._object.unexport();
        this._service.destroy();
        this._service = null;
        this._object = null;
    }
}
