    // The part of toast that lives in GNOME's shell. On Wayland a window
    // cannot say where it goes, so this puts toast's where they belong:
    // each stack in its corner, over everything, on every workspace, kept
    // there as it grows and shrinks, and never given the keyboard; and
    // the history's box in the middle, with it.
    //
    // The shell also keeps for itself the name every program sends its
    // notifications to, so this takes over from the shell's own handling
    // of them and hands each one on to toast. If toast cannot be reached,
    // the shell handles it as it always did.

    enable() {
        this._sized = new Map();
        this._before = null;
        this._mapped = global.window_manager.connect('map', (_wm, actor) => this._place(actor.meta_window));
        this._focus = global.display.connect('notify::focus-window', () => {
            const window = global.display.focus_window;
            if (window && !this._ours(window))
                this._before = window;
        });
        this._hide();
        this._forward();
        this._dbus = Gio.DBusExportedObject.wrapJSObject(SHELL_INTERFACE, this);
        this._dbus.export(Gio.DBus.session, '/io/github/jeffjose/Toast/Shell');
    }

    disable() {
        this._dbus.unexport();
        this._dbus = null;
        this._unforward();
        this._unhide();
        global.display.disconnect(this._focus);
        global.window_manager.disconnect(this._mapped);
        for (const [window, id] of this._sized)
            window.disconnect(id);
        this._sized = null;
        this._before = null;
    }

    _ours(window) {
        return window?.get_wm_class?.() === 'toast';
    }

    _place(window) {
        if (!window || !this._ours(window))
            return;
        const title = window.get_title() ?? '';
        if (title === 'toast history') {
            // Across the middle of the monitor in use, a fifth of the way
            // down, as clutch's box is; with the keyboard.
            const area = global.display.get_monitor_geometry(global.display.get_current_monitor());
            const frame = window.get_frame_rect();
            window.move_frame(true, area.x + Math.floor((area.width - frame.width) / 2), area.y + Math.floor(area.height / 5));
            window.make_above();
            Main.activateWindow(window);
            return;
        }
        const corner = title.replace(/^toast /, '');
        const monitor = global.display.get_current_monitor();
        const fit = () => {
            const area = Main.layoutManager.getWorkAreaForMonitor(monitor);
            const frame = window.get_frame_rect();
            let x = area.x + area.width - frame.width;
            if (corner.endsWith('left'))
                x = area.x;
            else if (corner === 'top' || corner === 'bottom')
                x = area.x + Math.floor((area.width - frame.width) / 2);
            const y = corner.startsWith('top') ? area.y : area.y + area.height - frame.height;
            window.move_frame(true, x, y);
        };
        window.make_above();
        window.stick();
        fit();
        if (!this._sized.has(window)) {
            this._sized.set(window, window.connect('size-changed', fit));
            const gone = window.connect('unmanaged', () => {
                window.disconnect(gone);
                const id = this._sized?.get(window);
                if (id !== undefined) {
                    window.disconnect(id);
                    this._sized.delete(window);
                }
            });
        }
        // Not to be given the keyboard: whatever had it has it back.
        if (global.display.focus_window === window && this._before)
            this._before.activate(global.get_current_time());
    }

    // Kept out of the window switcher and the overview, which leave out
    // what says it is to be skipped, and ask in JavaScript.
    _hide() {
        const proto = Meta.Window.prototype;
        const ours = window => this._ours(window);
        const skip = this._skip = proto.is_skip_taskbar;
        proto.is_skip_taskbar = function () {
            return ours(this) || skip.call(this);
        };
        try {
            this._skipProperty = Object.getOwnPropertyDescriptor(proto, 'skip_taskbar');
            Object.defineProperty(proto, 'skip_taskbar', {
                configurable: true,
                get() {
                    return ours(this) || skip.call(this);
                },
            });
            this._hidden = true;
        } catch (e) {
            console.log(`toast: windows cannot be kept from the switcher here: ${e}`);
        }
    }

    _unhide() {
        const proto = Meta.Window.prototype;
        proto.is_skip_taskbar = this._skip;
        if (this._hidden) {
            if (this._skipProperty)
                Object.defineProperty(proto, 'skip_taskbar', this._skipProperty);
            else
                delete proto.skip_taskbar;
        }
        this._hidden = false;
    }

    _forward() {
        // Where the shell keeps its own, which has differed between
        // versions of it.
        const shell = Main.notificationDaemon?._fdoNotificationDaemon;
        if (!shell?._dbusImpl) {
            console.log('toast: the shell keeps its notifications where toast cannot find them; it shows them itself');
            return;
        }
        this._shell = shell;
        shell._dbusImpl.unexport();
        const forward = (method, fallback) => (params, invocation) => {
            Gio.DBus.session.call(TOAST, PATH, FDO, method, invocation.get_parameters(), null, Gio.DBusCallFlags.NONE, -1, null, (bus, result) => {
                try {
                    invocation.return_value(bus.call_finish(result));
                } catch (e) {
                    // toast is not there: the shell's own, as before.
                    try {
                        fallback(params, invocation);
                    } catch (error) {
                        invocation.return_dbus_error('org.freedesktop.DBus.Error.Failed', `${error}`);
                    }
                }
            });
        };
        const handler = {
            NotifyAsync: forward('Notify', (params, invocation) => shell.NotifyAsync(params, invocation)),
            CloseNotificationAsync: forward('CloseNotification', (params, invocation) => {
                shell.CloseNotification(...params);
                invocation.return_value(null);
            }),
            GetCapabilitiesAsync: forward('GetCapabilities', (_params, invocation) => invocation.return_value(new GLib.Variant('(as)', [shell.GetCapabilities()]))),
            GetServerInformationAsync: forward('GetServerInformation', (_params, invocation) => invocation.return_value(new GLib.Variant('(ssss)', shell.GetServerInformation()))),
        };
        this._fdo = Gio.DBusExportedObject.wrapJSObject(FDO_INTERFACE, handler);
        this._fdo.export(Gio.DBus.session, PATH);
        // What toast says of them, said again as the shell, which is who
        // the programs are listening to.
        this._heard = Gio.DBus.session.signal_subscribe(TOAST, FDO, null, PATH, null, Gio.DBusSignalFlags.NONE,
            (_bus, _sender, _path, _interface, name, params) => this._fdo?.emit_signal(name, params));
    }

    _unforward() {
        if (!this._shell)
            return;
        Gio.DBus.session.signal_unsubscribe(this._heard);
        this._fdo.unexport();
        this._fdo = null;
        this._shell._dbusImpl.export(Gio.DBus.session, PATH);
        this._shell = null;
    }

    // What `toast status` asks, to see that this is running.
    State() {
        return JSON.stringify({version: VERSION, forwarding: !!this._shell, hidden: !!this._hidden});
    }
