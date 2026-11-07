// Inbox report: files in folders on the host.
//
// Reads a CSV from a folder on the host, counts its rows and totals its number columns in a js
// node, and writes a Markdown report to another folder; it also lists and serves the reports. The
// flow names its folders (inbox, reports), never a path: an admin binds each name to a real
// folder, read-only or read-write, and deploying asks for each use. A write lands whole when the
// run finishes, and the run log keeps what was read and a hash of what was written.
//
// Try it: first an admin binds the folders:
//   holon folder add inbox ~/holon-inbox --read-only
//   holon folder add reports ~/holon-reports
// Put sales.csv (lines region,amount / north,10 / south,32) in ~/holon-inbox, deploy it, then
//   curl -s -X POST -H 'content-type: application/json' -d '{"name":"sales.csv"}' \
//     http://127.0.0.1:8123/report                       rows 2
//   curl http://127.0.0.1:8123/reports/sales.csv.md      the report, amount 42
//   curl http://127.0.0.1:8123/reports                   lists it
import { flow, js, respond, route } from "holon"
import inbox from "folder:inbox"
import reports from "folder:reports"

flow("inbox-report", { port: 8123 })

// Summarizes a CSV: how many rows, and a Markdown table of each number column's total.
const summarize = js(
  function summarize(name, csv) {
    const lines = csv.split("\n").filter((line) => line.trim() !== "")
    if (lines.length === 0) throw new Error("the CSV has no header line")
    const header = lines[0].split(",").map((cell) => cell.trim())
    const records = lines.slice(1).map((line) => line.split(",").map((cell) => cell.trim()))
    const totals = header.flatMap((column, i) => {
      const numbers = records.map((record) => Number(record[i] ?? ""))
      const all = numbers.length > 0 && numbers.every((n) => !Number.isNaN(n))
      return all ? [`| ${column} | ${numbers.reduce((a, b) => a + b, 0)} |`] : []
    })
    const report = [
      `# ${name}`,
      "",
      `${records.length} rows.`,
      "",
      "| column | total |",
      "|---|---|",
      ...totals,
      "",
    ].join("\n")
    return { rows: records.length, report }
  },
  { outputs: ["rows", "report"] },
)

// Reads the named CSV from the inbox, writes its report to reports, and answers the report's name.
route.post("/report", (req) => {
  const name = req.json.name
  const summary = summarize(name, inbox[name])
  reports[`${name}.md`] = summary.report
  return { rows: summary.rows, report: `${name}.md` }
})

// The reports written so far, by file name.
route.get("/reports", () => reports.list())

// One report, as Markdown.
route.get("/reports/:name", (req) =>
  respond(reports[req.params.name], {
    headers: { "content-type": "text/markdown; charset=utf-8" },
  }),
)
