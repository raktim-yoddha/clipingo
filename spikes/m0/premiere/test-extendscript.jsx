// M0 spike, ExtendScript side (works on Premiere 2025/2026 while ExtendScript ships).
// Run with Premiere open on any project: VS Code + "ExtendScript Debugger" extension,
// target "Adobe Premiere Pro", run this file. Results: bundle/results-extendscript.txt
(function () {
    var dir = File($.fileName).parent.fsName + "/bundle";
    var log = [];
    function check(name, pass, info) {
        log.push((pass ? "PASS " : "FAIL ") + name + (info !== undefined ? " - " + info : ""));
    }
    function findSequence(prefix) {
        for (var i = 0; i < app.project.sequences.numSequences; i++)
            if (app.project.sequences[i].name.indexOf(prefix) === 0) return app.project.sequences[i];
        return null;
    }
    function findItem(item, pattern) {
        for (var i = 0; i < item.children.numItems; i++) {
            var child = item.children[i];
            if (pattern.test(child.name)) return child;
            if (child.type === ProjectItemType.BIN) {
                var hit = findItem(child, pattern);
                if (hit) return hit;
            }
        }
        return null;
    }

    try {
        check("premiere version", true, app.version);
        var bin = app.project.rootItem.createBin("Clipingo M0");
        var before = app.project.sequences.numSequences;
        check("importFiles(xml, srt) into open project",
            app.project.importFiles([dir + "/clipingo-m0.xml", dir + "/test.srt"], true, bin, false));

        var seq = findSequence("Clipingo M0");
        check("sequence added to the OPEN project (not a new project)", seq !== null,
            (app.project.sequences.numSequences - before) + " new sequence(s)");
        if (seq) {
            check("frame size 1080x1920", seq.frameSizeHorizontal == 1080 && seq.frameSizeVertical == 1920,
                seq.frameSizeHorizontal + "x" + seq.frameSizeVertical);
            check("V1 + V2 tracks", seq.videoTracks.numTracks >= 2, seq.videoTracks.numTracks + " video tracks");
            check("jump cut = 2 clips on V1", seq.videoTracks[0].clips.numItems == 2, seq.videoTracks[0].clips.numItems + " clips");
            check("markers", seq.markers.numMarkers >= 2, seq.markers.numMarkers + " markers");

            var clip = seq.videoTracks[1].clips[0];
            var comps = [];
            for (var c = 0; c < clip.components.numItems; c++) {
                var comp = clip.components[c], props = [];
                for (var p = 0; p < comp.properties.numItems; p++) {
                    var prop = comp.properties[p];
                    var keyed = false;
                    try { keyed = prop.isTimeVarying(); } catch (e) {}
                    props.push(prop.displayName + (keyed ? "*keyed*" : "=" + prop.getValue()));
                }
                comps.push(comp.displayName + " [" + props.join(", ") + "]");
            }
            check("V2 webcam clip effects (look for Crop + keyed Position/Scale)", true, comps.join(" | "));

            var srt = findItem(app.project.rootItem, /\.srt$/i);
            check("SRT imported as project item", srt !== null);
            if (srt) {
                var made = seq.createCaptionTrack(srt, 0, Sequence.CAPTION_FORMAT_SUBTITLE);
                check("createCaptionTrack(srt) -> native caption track", made);
            }
        }
    } catch (e) {
        check("script error", false, e.toString() + " line " + e.line);
    }

    var out = new File(dir + "/results-extendscript.txt");
    out.encoding = "UTF-8";
    out.open("w");
    out.write(log.join("\n"));
    out.close();
    return log.join("\n");
})();
