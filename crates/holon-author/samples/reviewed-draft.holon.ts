// Reviewed draft: an agent drafts what you ask for, and you approve it as it reads.
//
// You ask for a piece of writing by running the draft action yourself (the editor's run, or holon
// action), with its title and a brief. The run waits at an approval in two steps, a day at most:
// an agent bound to drafter takes the first, its draft the note the run is given, then a person
// bound to proofreader approves it as it reads. That step says starter: true, so you, who started
// the run, may take it: an output review, where the one who asked checks what was made. Left out,
// as in a payout, whoever started a run takes no step of it, and an agent that started one takes
// none whatever a step says. Each step is a different principal; one decline, with a reason,
// ends it. Approved, the run writes the draft to the folder drafts as <title>.md; declined, or
// undecided in time, nothing is written.
//
// A host may let no step take whoever started a run (holon access approvals --starter off): this
// flow then doesn't deploy, saying why, and a run already waiting goes by it at once.
//
// Before it works, an admin binds the folder and says who takes each step. On the server's machine
// holon signs in as you, the account that set the server up:
//   holon folder add drafts ~/holon-drafts
//   holon roles define drafter --allow run.decide --kinds agent
//   holon roles define proofreader --allow run.decide
//   holon people add quill --agent
//   holon people token quill                                          its token, printed once
//   holon bind drafter --to quill --flow reviewed-draft
//   holon bind proofreader --to <you> --flow reviewed-draft
// Its review then says who decides it: an agent holding drafter, then a person holding
// proofreader, who may be whoever started the run.
//
// Try it: deploy it, then
//   holon action reviewed-draft draft --input '{"title":"launch","brief":"two lines on Monday"}'
//   holon decide reviewed-draft                         the run, its steps and what it shows
// quill drafts it, with its token in HOLON_TOKEN, the draft as its note:
//   holon decide reviewed-draft <run> shape --approve --note '{"text":"We launch on Monday."}'
// then you read the draft (the editor's Approvals: ✓ drafter (quill) → ◻ proofreader (whoever
// started it may take it), quill's note beside it) and approve it, with HOLON_TOKEN unset:
//   holon decide reviewed-draft <run> shape --approve
// ~/holon-drafts/launch.md then holds the draft. Started by quill instead (holon action with its
// token), the run waits for someone else: an agent never decides a run it started.
import { action, approval, flow } from "holon"
import drafts from "folder:drafts"

flow("reviewed-draft")

// Asks for a draft: the run waits for the drafter's draft, then for a proofreader, who may be
// whoever asked for it.
export const draft = action(async (asked) => {
  const decision = await approval("shape", {
    steps: [
      { role: "drafter", kind: "agent" },
      { role: "proofreader", starter: true },
    ],
    expires: "1d",
    shows: { title: asked.title, brief: asked.brief },
  })
  // Declined (with why), or nobody decided in time: nothing is written.
  if (!decision.approved) {
    return { declined: decision.reason ?? "expired" }
  }
  // The draft is the drafter's note, as the proofreader approved it.
  drafts[`${asked.title}.md`] = decision.decisions[0].note.text
  return { written: `${asked.title}.md`, by: decision.by }
})
