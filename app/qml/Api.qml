// Promise-based calls into the Rust backend, and its events.
//   Api.call("get_settings").then(settings => ...).catch(message => ...)
//   Api.on("download-progress", payload => ...)   // returns an unsubscribe function

import ArcticHelper
import QtQuick
pragma Singleton

// Command names and camelCase argument names are the ones the Tauri
// frontend used; see app/src/app_*/commands.rs.
QtObject {
    id: api

    property var _pending: ({
    })
    property var _listeners: ({
    })
    property Connections _connections

    // Every backend event, for whoever wants all of them (the log).
    signal event(string name, var payload)

    function call(command, args) {
        return new Promise(function(resolve, reject) {
            const id = Backend.invoke(command, JSON.stringify(args || {
            }));
            api._pending[id] = {
                "resolve": resolve,
                "reject": reject
            };
        });
    }

    /// For the end of a chain that already has a `.catch`: `.then(Api.after(fn))`
    /// runs `fn` and passes the value on. (QML's Promise has no `finally`.)
    function after(fn) {
        return function(value) {
            fn();
            return value;
        };
    }

    function on(name, handler) {
        const list = api._listeners[name] || [];
        list.push(handler);
        api._listeners[name] = list;
        return function() {
            const current = api._listeners[name] || [];
            api._listeners[name] = current.filter((h) => {
                return h !== handler;
            });
        };
    }

    function _parse(json) {
        try {
            return JSON.parse(json);
        } catch (_) {
            return json;
        }
    }

    _connections: Connections {
        function onReplied(requestId, ok, json) {
            const pending = api._pending[requestId];
            if (!pending)
                return ;

            delete api._pending[requestId];
            const value = api._parse(json);
            if (ok)
                pending.resolve(value);
            else
                pending.reject(value);
        }

        function onBackendEvent(name, json) {
            const payload = api._parse(json);
            api.event(name, payload);
            (api._listeners[name] || []).forEach((handler) => {
                return handler(payload);
            });
        }

        target: Backend
    }

}
