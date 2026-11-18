
## inbound (unmatched targets: 0)
| cohort | mode | correct / n | expected, random matching | P(>= observed), random matching | expected, guess among feasible |
|---|---|---|---|---|---|
| c1_public | allowed | 0 / 5 | 1.0 | 1 | 1.00 |
| c2_addrsecret | allowed | 3 / 5 | 1.0 | 0.0917 | 1.00 |
| c3_fee | allowed | 0 / 5 | 1.0 | 1 | 0.50 |
| c4_self | allowed | 0 / 5 | 1.0 | 1 | 0.50 |
| main | allowed | 1 / 20 | 1.0 | 0.632 | 1.00 |
| c1_public | everything | 5 / 5 | 1.0 | 0.00833 | 1.00 |
| c2_addrsecret | everything | 5 / 5 | 1.0 | 0.00833 | 1.00 |
| c3_fee | everything | 0 / 5 | 1.0 | 1 | 0.50 |
| c4_self | everything | 0 / 5 | 1.0 | 1 | 0.50 |
| main | everything | 1 / 20 | 1.0 | 0.632 | 1.00 |

| feature | cohort | mean set kept / of | truth kept | unique right | unique wrong |
|---|---|---|---|---|---|
| timing | c1_public | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| timing | c2_addrsecret | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| timing | c3_fee | 1.00 / 10.00 | 1 / 5 | 1 | 4 |
| timing | c4_self | 1.00 / 10.00 | 0 / 5 | 0 | 5 |
| timing | main | 1.00 / 20.00 | 1 / 20 | 1 | 19 |
| gap | c1_public | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| gap | c2_addrsecret | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| gap | c3_fee | 1.00 / 10.00 | 1 / 5 | 1 | 4 |
| gap | c4_self | 1.00 / 10.00 | 0 / 5 | 0 | 5 |
| gap | main | 1.00 / 20.00 | 1 / 20 | 1 | 19 |
| bound | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| bound | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| bound | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| bound | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| bound | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| exact32 | c1_public | 1.20 / 5.00 | 1 / 5 | 1 | 3 |
| exact32 | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| exact32 | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact32 | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact32 | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| exact20 | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| exact20 | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| exact20 | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact20 | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact20 | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| sub8 | c1_public | 1.20 / 5.00 | 1 / 5 | 1 | 3 |
| sub8 | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| sub8 | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| sub8 | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| sub8 | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| derivedC | c1_public | 1.00 / 5.00 | 5 / 5 | 5 | 0 |
| derivedC | c2_addrsecret | 1.00 / 5.00 | 5 / 5 | 5 | 0 |
| derivedC | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| derivedC | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| derivedC | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| fee | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| fee | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| fee | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| fee | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| fee | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| size | c1_public | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| size | c2_addrsecret | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| size | c3_fee | 2.00 / 10.00 | 1 / 5 | 0 | 0 |
| size | c4_self | 2.00 / 10.00 | 1 / 5 | 0 | 0 |
| size | main | 3.00 / 20.00 | 3 / 20 | 0 | 0 |

## outbound (unmatched targets: 0)
| cohort | mode | correct / n | expected, random matching | P(>= observed), random matching | expected, guess among feasible |
|---|---|---|---|---|---|
| c1_public | allowed | 0 / 5 | 1.0 | 1 | 1.00 |
| c2_addrsecret | allowed | 3 / 5 | 1.0 | 0.0917 | 1.00 |
| c3_fee | allowed | 0 / 5 | 1.0 | 1 | 0.50 |
| c4_self | allowed | 1 / 5 | 1.0 | 0.633 | 0.50 |
| main | allowed | 0 / 20 | 1.0 | 1 | 1.00 |
| c1_public | everything | 0 / 5 | 1.0 | 1 | 1.00 |
| c2_addrsecret | everything | 3 / 5 | 1.0 | 0.0917 | 1.00 |
| c3_fee | everything | 0 / 5 | 1.0 | 1 | 0.50 |
| c4_self | everything | 1 / 5 | 1.0 | 0.633 | 0.50 |
| main | everything | 0 / 20 | 1.0 | 1 | 1.00 |

| feature | cohort | mean set kept / of | truth kept | unique right | unique wrong |
|---|---|---|---|---|---|
| timing | c1_public | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| timing | c2_addrsecret | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| timing | c3_fee | 1.00 / 10.00 | 1 / 5 | 1 | 4 |
| timing | c4_self | 1.00 / 10.00 | 0 / 5 | 0 | 5 |
| timing | main | 1.00 / 20.00 | 1 / 20 | 1 | 19 |
| gap | c1_public | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| gap | c2_addrsecret | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| gap | c3_fee | 1.00 / 10.00 | 1 / 5 | 1 | 4 |
| gap | c4_self | 1.00 / 10.00 | 0 / 5 | 0 | 5 |
| gap | main | 1.00 / 20.00 | 1 / 20 | 1 | 19 |
| bound | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| bound | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| bound | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| bound | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| bound | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| exact32 | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| exact32 | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| exact32 | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact32 | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact32 | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| exact20 | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| exact20 | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| exact20 | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact20 | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact20 | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| sub8 | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| sub8 | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| sub8 | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| sub8 | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| sub8 | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| derivedNf | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| derivedNf | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| derivedNf | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| derivedNf | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| derivedNf | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| fee | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| fee | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| fee | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| fee | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| fee | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| size | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| size | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| size | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| size | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| size | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |

## lez (unmatched targets: 0)
| cohort | mode | correct / n | expected, random matching | P(>= observed), random matching | expected, guess among feasible |
|---|---|---|---|---|---|
| c1_public | allowed | 1 / 5 | 1.0 | 0.633 | 1.00 |
| c2_addrsecret | allowed | 0 / 5 | 1.0 | 1 | 1.00 |
| c3_fee | allowed | 0 / 5 | 1.0 | 1 | 0.50 |
| c4_self | allowed | 1 / 5 | 1.0 | 0.633 | 0.50 |
| main | allowed | 0 / 20 | 1.0 | 1 | 1.00 |
| c1_public | everything | 5 / 5 | 1.0 | 0.00833 | 1.00 |
| c2_addrsecret | everything | 1 / 5 | 1.0 | 0.633 | 1.00 |
| c3_fee | everything | 0 / 5 | 1.0 | 1 | 0.50 |
| c4_self | everything | 0 / 5 | 1.0 | 1 | 0.50 |
| main | everything | 0 / 20 | 1.0 | 1 | 1.00 |

| feature | cohort | mean set kept / of | truth kept | unique right | unique wrong |
|---|---|---|---|---|---|
| timing | c1_public | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| timing | c2_addrsecret | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| timing | c3_fee | 1.00 / 10.00 | 0 / 5 | 0 | 5 |
| timing | c4_self | 1.00 / 10.00 | 1 / 5 | 1 | 4 |
| timing | main | 1.00 / 20.00 | 1 / 20 | 1 | 19 |
| gap | c1_public | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| gap | c2_addrsecret | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| gap | c3_fee | 1.00 / 10.00 | 0 / 5 | 0 | 5 |
| gap | c4_self | 1.00 / 10.00 | 1 / 5 | 1 | 4 |
| gap | main | 1.00 / 20.00 | 1 / 20 | 1 | 19 |
| exact32 | c1_public | 1.00 / 5.00 | 5 / 5 | 5 | 0 |
| exact32 | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| exact32 | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact32 | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact32 | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| sub8 | c1_public | 1.00 / 5.00 | 5 / 5 | 5 | 0 |
| sub8 | c2_addrsecret | 3.60 / 5.00 | 4 / 5 | 0 | 1 |
| sub8 | c3_fee | 8.20 / 10.00 | 4 / 5 | 0 | 1 |
| sub8 | c4_self | 8.20 / 10.00 | 4 / 5 | 0 | 1 |
| sub8 | main | 16.20 / 20.00 | 16 / 20 | 0 | 4 |
| fee | c1_public | 1.00 / 5.00 | 5 / 5 | 5 | 0 |
| fee | c2_addrsecret | 2.80 / 5.00 | 3 / 5 | 0 | 2 |
| fee | c3_fee | 8.20 / 10.00 | 4 / 5 | 0 | 1 |
| fee | c4_self | 6.40 / 10.00 | 3 / 5 | 0 | 2 |
| fee | main | 10.70 / 20.00 | 10 / 20 | 0 | 8 |

## roundtrip (unmatched targets: 0)
| cohort | mode | correct / n | expected, random matching | P(>= observed), random matching | expected, guess among feasible |
|---|---|---|---|---|---|
| c1_public | allowed | 0 / 5 | 1.0 | 1 | 1.00 |
| c2_addrsecret | allowed | 0 / 5 | 1.0 | 1 | 1.00 |
| c3_fee | allowed | 0 / 5 | 1.0 | 1 | 0.50 |
| c4_self | allowed | 1 / 5 | 1.0 | 0.633 | 0.50 |
| main | allowed | 1 / 20 | 1.0 | 0.632 | 1.00 |
| c1_public | everything | 0 / 5 | 1.0 | 1 | 1.00 |
| c2_addrsecret | everything | 0 / 5 | 1.0 | 1 | 1.00 |
| c3_fee | everything | 5 / 5 | 1.0 | 0.00833 | 0.50 |
| c4_self | everything | 5 / 5 | 1.0 | 0.00833 | 0.50 |
| main | everything | 1 / 20 | 1.0 | 0.632 | 1.00 |

| feature | cohort | mean set kept / of | truth kept | unique right | unique wrong |
|---|---|---|---|---|---|
| timing | c1_public | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| timing | c2_addrsecret | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| timing | c3_fee | 1.00 / 10.00 | 1 / 5 | 1 | 4 |
| timing | c4_self | 1.00 / 10.00 | 0 / 5 | 0 | 5 |
| timing | main | 1.00 / 20.00 | 1 / 20 | 1 | 19 |
| gap | c1_public | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| gap | c2_addrsecret | 1.00 / 5.00 | 1 / 5 | 1 | 4 |
| gap | c3_fee | 1.00 / 10.00 | 1 / 5 | 1 | 4 |
| gap | c4_self | 1.00 / 10.00 | 0 / 5 | 0 | 5 |
| gap | main | 1.00 / 20.00 | 1 / 20 | 1 | 19 |
| sender | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| sender | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| sender | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| sender | c4_self | 1.00 / 10.00 | 5 / 5 | 5 | 0 |
| sender | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| exact32 | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| exact32 | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| exact32 | c3_fee | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| exact32 | c4_self | 1.00 / 10.00 | 5 / 5 | 5 | 0 |
| exact32 | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
| fee | c1_public | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| fee | c2_addrsecret | 5.00 / 5.00 | 5 / 5 | 0 | 0 |
| fee | c3_fee | 1.00 / 10.00 | 5 / 5 | 5 | 0 |
| fee | c4_self | 10.00 / 10.00 | 5 / 5 | 0 | 0 |
| fee | main | 20.00 / 20.00 | 20 / 20 | 0 | 0 |
