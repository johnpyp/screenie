// Who may use the extension: screenie, as GNOME knows it by its desktop entry (the
// binary its Exec runs), and nothing sandboxed. KWin grants its screenshots the same way.

import Gio from 'gi://Gio';
import GioUnix from 'gi://GioUnix';
import GLib from 'gi://GLib';

const APP = 'dev.johnpyp.Screenie.desktop';

Gio._promisify(Gio.DBusConnection.prototype, 'call');

/**
 * The caller's process id, if it's screenie.
 *
 * @param {string} sender - the caller's unique bus name
 * @returns {Promise<number | null>}
 */
export async function screeniePid(sender) {
    const reply = await Gio.DBus.session.call(
        'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
        'GetConnectionUnixProcessID', new GLib.Variant('(s)', [sender]),
        new GLib.VariantType('(u)'), Gio.DBusCallFlags.NONE, -1, null);
    const [pid] = reply.deepUnpack();
    // As GNOME's own restricted interfaces are in unsafe mode (for testing).
    if (global.context.unsafe_mode)
        return pid;
    if (GLib.file_test(`/proc/${pid}/root/.flatpak-info`, GLib.FileTest.EXISTS))
        return null;
    const binary = screenieBinary();
    if (!binary)
        return null;
    const running = fileId(`/proc/${pid}/exe`);
    return running !== null && running === fileId(binary) ? pid : null;
}

/** The binary screenie's desktop entry runs, if it has one. */
function screenieBinary() {
    const entry = GioUnix.DesktopAppInfo.new(APP);
    const executable = entry?.get_executable();
    if (!executable)
        return null;
    return GLib.path_is_absolute(executable)
        ? executable
        : GLib.find_program_in_path(executable);
}

/**
 * The file a path leads to (links followed), as device and inode.
 *
 * @param {string} path - the path
 * @returns {string | null}
 */
function fileId(path) {
    try {
        const info = Gio.File.new_for_path(path).query_info(
            'unix::device,unix::inode', Gio.FileQueryInfoFlags.NONE, null);
        return `${info.get_attribute_uint32('unix::device')}:${info.get_attribute_uint64('unix::inode')}`;
    } catch {
        return null;
    }
}
