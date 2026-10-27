// M0 spike, UXP side (Premiere 25.6+). Load with UXP Developer Tool, open the panel,
// paste the bundle folder path, press Run. Every step is independent and logs PASS/FAIL,
// so a wrong API name shows up as a FAIL with the error instead of stopping the run.
const ppro = require("premierepro");
const fs = require("fs");

const out = document.getElementById("out");
const lines = [];
const log = (pass, name, info) => {
  lines.push(`${pass ? "PASS" : "FAIL"} ${name}${info !== undefined ? " - " + info : ""}`);
  out.textContent = lines.join("\n");
};
async function step(name, fn) {
  try {
    const info = await fn();
    log(info !== false, name, info === true || info === false ? undefined : info);
  } catch (e) {
    log(false, name, e && e.message ? e.message : String(e));
  }
}

document.getElementById("run").addEventListener("click", async () => {
  lines.length = 0;
  const dir = document.getElementById("dir").value.trim().replace(/[\\/]+$/, "");
  let project, sequences;

  await step("active project", async () => {
    project = await ppro.Project.getActiveProject();
    return project ? project.name : false;
  });
  await step("importFiles(xml, srt) into open project", async () => {
    const before = (await project.getSequences()).length;
    const ok = await project.importFiles([`${dir}\\clipingo-m0.xml`, `${dir}\\test.srt`], true);
    sequences = await project.getSequences();
    return `${ok}, sequences ${before} -> ${sequences.length}`;
  });
  await step("sequence 'Clipingo M0 Short' present", async () => {
    const names = sequences.map((s) => s.name);
    return names.some((n) => n.startsWith("Clipingo M0")) ? names.join(", ") : false;
  });
  await step("caption tracks on that sequence (read API)", async () => {
    const seq = sequences.find((s) => s.name.startsWith("Clipingo M0"));
    return `${await seq.getCaptionTrackCount()} caption track(s)`;
  });
  await step("UXP has a create-caption-track call", async () => {
    const seq = sequences.find((s) => s.name.startsWith("Clipingo M0"));
    const found = Object.getOwnPropertyNames(Object.getPrototypeOf(seq)).filter((n) => /caption/i.test(n));
    return found.length ? found.join(", ") : false;
  });
  await step("Transcript.exportToJSON (learn the format)", async () => {
    // Needs a clip that Premiere already transcribed (Text panel → Transcribe) somewhere in the project.
    const root = await project.getRootItem();
    const queue = [...(await root.getItems())];
    while (queue.length) {
      const item = queue.shift();
      const folder = ppro.FolderItem.cast(item);
      if (folder) {
        queue.push(...(await folder.getItems()));
        continue;
      }
      const clip = ppro.ClipProjectItem.cast(item);
      if (!clip) continue;
      const json = await ppro.Transcript.exportToJSON(clip);
      if (json) {
        fs.writeFileSync(`${dir}\\transcript-format.json`, json);
        return `saved ${json.length} chars from "${item.name}" to transcript-format.json`;
      }
    }
    return "no transcribed clip found: transcribe test.mp4 in Premiere's Text panel and run again";
  });

  try {
    fs.writeFileSync(`${dir}\\results-uxp.txt`, lines.join("\n"));
  } catch (e) {
    log(false, "write results-uxp.txt", e.message);
  }
});
