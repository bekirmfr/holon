// Payment batch: XML both ways, a pain.001 written and a pain.002 read.
//
// POST /batch takes a CSV of payouts (id, name, iban, amount, reference) and answers the
// pain.001.001.09 credit transfer a bank takes for it, debited from the account below and executed
// on the date the request gives, or the next business day at Frankfurt's offset. Each amount is
// rounded to cents and the control sum is their exact sum, as decimals written as text. The
// message id is a hash of the file and its date, so the same file sent twice for one day has one
// id, and the bank refuses the second. The schema fixes the order of every element, so an element
// is a list: its name, a record of its attributes, its children in order. What the schema would
// refuse (an id past 35 characters, a name or a reference empty or past 140, an IBAN not in its
// shape, a date that isn't one) answers 400 here, not at the bank.
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

// What pain.001.001.09 takes of a payout: an id of 1 to 35 characters, a name and a reference of
// 1 to 140, an IBAN in its shape, and an amount more than 0.
function takes(p) {
  return (
    fits(p.id, 35) &&
    fits(p.name, 140) &&
    fits(p.reference, 140) &&
    isIban(p.iban) &&
    decimal_compare(p.amount, "0") > 0
  )
}

// A text of 1 to most characters, as the schema's Max35Text and Max140Text.
function fits(s: string, most: number) {
  return s.length >= 1 && s.length <= most
}

// An IBAN in the schema's shape: two capitals, two digits, then 1 to 30 letters or digits.
function isIban(s: string) {
  return (
    s.length >= 5 &&
    s.length <= 34 &&
    s.slice(0, 2).split("").every(isCapital) &&
    s.slice(2, 4).split("").every(isDigit) &&
    s.slice(4).split("").every(isLetterOrDigit)
  )
}

function isCapital(c: string) {
  return c >= "A" && c <= "Z"
}

function isDigit(c: string) {
  return c >= "0" && c <= "9"
}

function isLetterOrDigit(c: string) {
  return isCapital(c) || isDigit(c) || (c >= "a" && c <= "z")
}

// A CSV of payouts as a pain.001, or 400 for a file the schema wouldn't take.
route.post("/batch", (req) => {
  let file
  try {
    const payouts = csv_parse(req.body).map((p) => ({ ...p, amount: decimal_round(p.amount, 2) }))
    const created = now * 1000
    const asked =
      req.query.date ?? business_day_add(date_text(created, "+01:00", { format: "date" }), 1)
    const date = date_text(asked, "Z", { format: "date" })
    const sum = decimal_sum(payouts.map((p) => p.amount))
    // A control sum has 18 digits at most, 2 of them cents; a date is a calendar day, as written.
    if (
      payouts.length === 0 ||
      !payouts.every(takes) ||
      decimal_compare(sum, "10000000000000000") >= 0 ||
      date !== asked
    ) {
      throw new Error("not a batch a pain.001 takes")
    }
    const id = sha256(date + req.body).slice(0, 32)
    const document = [
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
    file = xml_format(document, { layout: "indented" })
  } catch {
    return status(400, {
      error:
        "not a CSV of payouts a pain.001 takes: id,name,iban,amount,reference, each id of 1 to 35 characters, each name and reference of 1 to 140, each IBAN as one is written, each amount more than 0, a date as YYYY-MM-DD",
    })
  }
  return respond(file, { headers: { "content-type": "application/xml" } })
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
