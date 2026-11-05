// fmt's input: a flow written any which way (lang/format.rs's tests lay it out).
import {flow,route,action,event,status,respond,js,every} from 'holon'
import state from "state"
import notes from "collection:notes"
import { charge } from "flow:billing"
flow("messy",{port:8000})
// a function
function fee(total:number,vip){
  if(total>100&&vip===true){return 0}
  else if (total > 50) return 1
  else { return 5 }
}
route.post("/order",async (req)=>{
    const {items,who}=req.json
    let total=req.json.base
    switch(who){
        case "vip": total=total*2; break
        // the rest
        default:
          total=total+2
    }
    if (req.json.coupon === "X-COUPON-ONE" || req.json.coupon === "X-COUPON-TWO" || req.json.coupon === "X-THREE") total = total - 1
    const paid=await charge({total:total+fee(total,who==="vip"),items:items,who:who,note:"a long enough note to break"})
    return (paid.answer ?? 0) > 0 && paid.run > 0 && paid.message === "none" && total > 0 && who !== "nobody"
},{log:'lazy'})
export const pay=action(async(order)=>{const paid=await event("paid",{expires:"1d"});return paid.method})
every("1m",()=>{state.ticks=(state.ticks??0)+1})
