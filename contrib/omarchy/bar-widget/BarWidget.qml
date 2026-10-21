// nuncio unread badge for the Omarchy 4 bar (Quickshell).
//
// Shows how many WhatsApp messages are unread, as the nuncio daemon reports it in the
// `UnreadCount` property of its session-bus object. Only that number ever reaches the
// bar: no chat names, no message text. Hidden at 0, and hidden while the daemon is not
// running. A click opens nuncio-desktop, or focuses it when it is already open.
//
// Quickshell has no generic D-Bus module (only service-specific ones such as Mpris or
// UPower), so the widget asks `busctl`, which ships with systemd:
//   - `busctl get-property` reads the count; `--auto-start=no` keeps the bar from
//     starting the daemon through D-Bus activation just to read a number.
//   - `busctl monitor` watches the daemon's object for `UnreadChanged` and
//     `PropertiesChanged` signals and triggers a read when one arrives.
//   - A 30 s poll covers what the signals do not yet: until nunciod emits
//     UnreadChanged and keeps UnreadCount live (handoff T18), the property is the
//     count at the daemon's start, and the poll is what picks up a restart.
import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

BarWidget {
  id: root
  moduleName: "pwn27sbx.nuncio-unread"

  // The daemon's D-Bus face (proto/wire.md, "D-Bus face").
  readonly property string busName: "io.github.pwn27sbx.nuncio"
  readonly property string objectPath: "/io/github/pwn27sbx/nuncio"
  readonly property string interfaceName: "io.github.pwn27sbx.nuncio"
  // nuncio-desktop's window class (its GTK application id, which differs from the
  // daemon's bus name above), for launch-or-focus.
  readonly property string windowClass: "^io\\.github\\.pwn27sbx\\.nuncio\\.Desktop$"

  property int unread: 0
  property bool readAgain: false

  // The pause before the monitor is started again: 5 s, doubling after every relaunch up
  // to 5 min, and back to 5 s once the monitor prints a line (so it is really running).
  readonly property int monitorDelayMin: 5000
  readonly property int monitorDelayMax: 300000
  property int monitorDelay: monitorDelayMin

  function refresh() {
    if (readProc.running) {
      readAgain = true
    } else {
      readProc.running = true
    }
  }

  // busctl prints an int32 property as `i 3`. Anything else (the daemon is not
  // running, the name is not owned, a parse failure) reads as nothing unread.
  function parseCount(text) {
    var match = /^i\s+(-?\d+)\s*$/.exec(String(text || "").trim())
    var n = match ? parseInt(match[1], 10) : 0
    return n > 0 ? n : 0
  }

  function openApp() {
    Util.execArgv(["omarchy-launch-or-focus", root.windowClass, "uwsm-app -- nuncio-desktop"])
  }

  visible: unread > 0
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  Process {
    id: readProc
    command: ["busctl", "--user", "--auto-start=no", "get-property",
      root.busName, root.objectPath, root.interfaceName, "UnreadCount"]
    stdout: StdioCollector {
      onStreamFinished: root.unread = root.parseCount(text)
    }
    onExited: function(exitCode) {
      if (exitCode !== 0) root.unread = 0
      if (root.readAgain) {
        root.readAgain = false
        root.refresh()
      }
    }
  }

  Process {
    id: monitorProc
    running: true
    command: ["busctl", "--user", "monitor",
      "--match", "type='signal',path='" + root.objectPath + "'"]
    stdout: SplitParser {
      onRead: function(line) {
        root.monitorDelay = root.monitorDelayMin
        var s = String(line)
        if (s.indexOf("UnreadChanged") !== -1 || s.indexOf("PropertiesChanged") !== -1) root.refresh()
      }
    }
    // A monitor that ends (the bus restarted, busctl refused the match) is started again
    // after a pause that backs off while it keeps failing; the poll keeps the badge right
    // meanwhile.
    onExited: {
      restartMonitor.interval = root.monitorDelay
      restartMonitor.restart()
      root.monitorDelay = Math.min(root.monitorDelay * 2, root.monitorDelayMax)
    }
  }

  Timer {
    id: restartMonitor
    interval: root.monitorDelayMin
    onTriggered: monitorProc.running = true
  }

  Timer {
    interval: 30000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: root.refresh()
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: ""
    labelText: root.unread > 99 ? "99+" : String(root.unread)
    tooltipText: root.unread === 1 ? "1 unread WhatsApp message" : root.unread + " unread WhatsApp messages"
    onPressed: root.openApp()
  }
}
