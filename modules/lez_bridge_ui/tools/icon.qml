// Renders src/icons/icon.png (256 px: the Logos package icon contract, logos-package spec.md), the mark: a white η on an orange rounded tile.
//   qml tools/icon.qml -- <out.png>
import QtQuick
Rectangle {
    width: 256; height: 256; color: "transparent"
    Rectangle {
        anchors.fill: parent; anchors.margins: 8; radius: 60; color: "#e8662f"
        Text { anchors.centerIn: parent; anchors.verticalCenterOffset: 12; text: "η"; color: "#ffffff"; font.pixelSize: 176; font.weight: Font.DemiBold }
    }
    Timer { interval: 300; running: true; onTriggered: parent.grabToImage(function(r) { r.saveToFile(Qt.application.arguments[Qt.application.arguments.length - 1]); Qt.quit() }) }
}
