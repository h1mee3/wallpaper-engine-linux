import QtQuick
import QtMultimedia
import org.kde.plasma.plasmoid

WallpaperItem {
    id: root

    readonly property string videoPath: root.configuration.VideoPath ?? ""
    readonly property string imagePath: root.configuration.ImagePath ?? ""
    readonly property string scalingMode: root.configuration.ScalingMode ?? "fill"
    readonly property bool isMuted: root.configuration.Muted ?? true
    readonly property int volumeLevel: root.configuration.Volume ?? 50
    readonly property bool isPaused: root.configuration.Paused ?? false

    Rectangle {
        anchors.fill: parent
        color: "#000000"
    }

    AudioOutput {
        id: audioOutput
        muted: root.isMuted
        volume: root.volumeLevel / 100.0
    }

    MediaPlayer {
        id: player
        audioOutput: audioOutput
        loops: MediaPlayer.Infinite
        source: {
            if (!root.videoPath) return "";
            return root.videoPath.startsWith("file://") ? root.videoPath : Qt.resolvedUrl("file://" + root.videoPath);
        }
        Component.onCompleted: {
            if (source != "") {
                if (root.isPaused) {
                    pause();
                } else {
                    play();
                }
            }
        }
        onSourceChanged: {
            if (source != "") {
                if (root.isPaused) {
                    pause();
                } else {
                    play();
                }
            } else {
                stop();
            }
        }
        onMediaStatusChanged: {
            if ((mediaStatus === MediaPlayer.LoadedMedia || mediaStatus === MediaPlayer.BufferedMedia) && !root.isPaused) {
                play();
            }
        }
    }

    onIsPausedChanged: {
        if (isPaused) {
            player.pause();
        } else {
            player.play();
        }
    }

    VideoOutput {
        id: videoOutput
        anchors.fill: parent
        visible: player.hasVideo
        fillMode: {
            if (root.scalingMode === "fit") return VideoOutput.PreserveAspectFit;
            if (root.scalingMode === "stretch") return VideoOutput.Stretch;
            return VideoOutput.PreserveAspectCrop;
        }
    }

    Image {
        id: fallbackImage
        anchors.fill: parent
        visible: !player.hasVideo && source != ""
        fillMode: Image.PreserveAspectCrop
        source: {
            if (!root.imagePath) return "";
            return root.imagePath.startsWith("file://") ? root.imagePath : Qt.resolvedUrl("file://" + root.imagePath);
        }
    }
}
