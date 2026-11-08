// Payment batch: XML both ways, a pain.001 written and a pain.002 read.
//
// POST /batch takes a CSV of payouts (id, name, iban, amount, reference) and answers the
// pain.001.001.09 credit transfer a bank takes for it, debited from the account below and executed
// on the date the request gives, or the next business day at Frankfurt's offset. Each amount is
// rounded to cents and the control sum is their exact sum, as decimals written as text. The
// message id is a hash of the file and its date, so the same file sent twice for one day has one
// id, and the bank refuses the second. The schema fixes the order of every element, so an element
// is a list: its name, a record of its attributes, its children in order.
//
// POST /status reads the bank's pain.002 status report back: the group's status, and each
// payment's with the reason a rejected one gives.
//
// Try it: deploy it, save this as payouts.csv
//   id,name,iban,amount,reference
//   P1,Jane Roe,FR1420041010050500013M02606,100,Invoice 42
//   P2,John Doe,NL91ABNA0417164300,0.10,Refund 7
//   P3,Max Muster,DE75512108001245126199,0.2,Refund 8
// then
//   curl --data-binary @payouts.csv 'http://127.0.0.1:8182/batch?date=2026-11-10'
// answers the pain.001, three payments whose control sum is 100.30.
import {
  business_day_add,
  csv_parse,
  date_text,
  decimal_compare,
  decimal_round,
  decimal_sum,
  flow,
  respond,
  route,
  sha256,
  status,
  xml_attribute,
  xml_find,
  xml_format,
  xml_parse,
  xml_text,
} from "holon"
import { now } from "holon:clock"

flow("payment-batch", { port: 8182 })

const DEBTOR = "Acme Payouts GmbH"
const DEBTOR_IBAN = "DE89370400440532013000"
const DEBTOR_BIC = "COBADEFFXXX"

// One payout as a credit transfer: its id end to end, its amount in euros, who is paid and why.
function transfer(p) {
  return [
    "CdtTrfTxInf",
    ["PmtId", ["EndToEndId", p.id]],
    ["Amt", ["InstdAmt", { Ccy: "EUR" }, p.amount]],
    ["Cdtr", ["Nm", p.name]],
    ["CdtrAcct", ["Id", ["IBAN", p.iban]]],
    ["RmtInf", ["Ustrd", p.reference]],
  ]
}

// A CSV of payouts as a pain.001, or 400 for a file that isn't one.
route.post("/batch", (req) => {
  let document
  try {
    const payouts = csv_parse(req.body).map((p) => ({ ...p, amount: decimal_round(p.amount, 2) }))
    if (payouts.length === 0 || !payouts.every((p) => decimal_compare(p.amount, "0") > 0)) {
      throw new Error("a batch has payouts, each more than 0")
    }
    const created = now * 1000
    const date =
      req.query.date ?? business_day_add(date_text(created, "+01:00", { format: "date" }), 1)
    const id = sha256(date + req.body).slice(0, 32)
    const sum = decimal_sum(payouts.map((p) => p.amount))
    document = [
      "Document",
      { xmlns: "urn:iso:std:iso:20022:tech:xsd:pain.001.001.09" },
      [
        "CstmrCdtTrfInitn",
        [
          "GrpHdr",
          ["MsgId", id],
          ["CreDtTm", date_text(created, "+01:00")],
          ["NbOfTxs", payouts.length],
          ["CtrlSum", sum],
          ["InitgPty", ["Nm", DEBTOR]],
        ],
        [
          "PmtInf",
          ["PmtInfId", id],
          ["PmtMtd", "TRF"],
          ["NbOfTxs", payouts.length],
          ["CtrlSum", sum],
          ["PmtTpInf", ["SvcLvl", ["Cd", "SEPA"]]],
          ["ReqdExctnDt", ["Dt", date]],
          ["Dbtr", ["Nm", DEBTOR]],
          ["DbtrAcct", ["Id", ["IBAN", DEBTOR_IBAN]]],
          ["DbtrAgt", ["FinInstnId", ["BICFI", DEBTOR_BIC]]],
          ["ChrgBr", "SLEV"],
          ...payouts.map(transfer),
        ],
      ],
    ]
  } catch {
    return status(400, {
      error: "not a CSV of payouts (id,name,iban,amount,reference), each more than 0",
    })
  }
  return respond(xml_format(document, { layout: "indented" }), {
    headers: { "content-type": "application/xml" },
  })
})

// The bank's pain.002 status report: the group's status, and each payment's.
route.post("/status", (req) => {
  const report = xml_parse(req.body)
  if (
    !(xml_attribute(report, "xmlns") ?? "").startsWith("urn:iso:std:iso:20022:tech:xsd:pain.002.")
  ) {
    return status(400, { error: "not a pain.002 status report" })
  }
  return {
    group: xml_text(report, "CstmrPmtStsRpt/OrgnlGrpInfAndSts/GrpSts") ?? "",
    payments: xml_find(report, "CstmrPmtStsRpt/OrgnlPmtInfAndSts/TxInfAndSts").map((tx) => ({
      id: xml_text(tx, "OrgnlEndToEndId"),
      status: xml_text(tx, "TxSts"),
      reason: xml_text(tx, "StsRsnInf/Rsn/Cd") ?? "",
    })),
  }
})
