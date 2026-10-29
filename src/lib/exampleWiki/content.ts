// SPDX-License-Identifier: AGPL-3.0-only
// Generated content for the example wiki: one person's notes, chats and
// documents while getting v2 of her app out. Simplified Chinese is converted
// from Traditional Chinese.
import type { ExampleWikiContent } from "./types";

export const EXAMPLE_WIKI: ExampleWikiContent = {
  "sources": [
    {
      "id": "ex-m-sync-1014",
      "kind": "notes",
      "agent": "obsidian",
      "space": "Launch",
      "day": 14,
      "title": {
        "en": "Launch sync, Oct 14",
        "zh-Hant": "上線例會，10 月 14 日",
        "zh-Hans": "上线例会，10 月 14 日"
      },
      "body": {
        "en": "Target launch date: October 24. Price stays at $8 a month until the customer interviews are in. Maya will share the interview notes on Friday.",
        "zh-Hant": "目標上線日：10 月 24 日。客戶訪談結果出來前，價格維持每月 8 美元。Maya 週五會分享訪談筆記。",
        "zh-Hans": "目标上线日：10 月 24 日。客户访谈结果出来前，价格维持每月 8 美元。Maya 周五会分享访谈笔记。"
      }
    },
    {
      "id": "ex-m-sync-1021",
      "kind": "notes",
      "agent": "obsidian",
      "space": "Launch",
      "day": 21,
      "title": {
        "en": "Launch sync, Oct 21",
        "zh-Hant": "上線例會，10 月 21 日",
        "zh-Hans": "上线例会，10 月 21 日"
      },
      "body": {
        "en": "Launch moves to October 31: the payment provider needs time to review our new merchant account. Price set at $10 a month, with 20% off the yearly plan. Pam owns the payment review. Dev owns the release notes.",
        "zh-Hant": "上線改到 10 月 31 日：付款服務商要審核我們新的商家帳號。價格定為每月 10 美元，年繳打八折。Pam 負責付款審核，Dev 負責版本說明。",
        "zh-Hans": "上线改到 10 月 31 日：付款服务商要审核我们新的商家账号。价格定为每月 10 美元，年缴打八折。Pam 负责付款审核，Dev 负责版本说明。"
      }
    },
    {
      "id": "ex-m-retro",
      "kind": "notes",
      "agent": "obsidian",
      "space": "Research",
      "day": 7,
      "title": {
        "en": "Retro, Oct 7",
        "zh-Hant": "回顧會議，10 月 7 日",
        "zh-Hans": "回顾会议，10 月 7 日"
      },
      "body": {
        "en": "Half of new sign-ups never finish setup. Most of them stop at the bank connection step. Sofia says the same question comes up in support every day.",
        "zh-Hant": "一半的新註冊用戶沒有完成設定，大多停在連接銀行那一步。Sofia 說客服每天都會收到同樣的問題。",
        "zh-Hans": "一半的新注册用户没有完成设置，大多停在连接银行那一步。Sofia 说客服每天都会收到同样的问题。"
      }
    },
    {
      "id": "ex-c-pricing",
      "kind": "chat",
      "agent": "chatgpt",
      "space": "Launch",
      "day": 16,
      "title": {
        "en": "ChatGPT: pricing options",
        "zh-Hant": "ChatGPT：定價選項",
        "zh-Hans": "ChatGPT：定价选项"
      },
      "body": {
        "en": "Compared $8, $10 and $12 a month. At $10, the yearly plan with 20% off comes to $96, just under the $100 a year customers said they would pay.",
        "zh-Hant": "比較了每月 8、10、12 美元。定在 10 美元時，年繳打八折是 96 美元，剛好低於客戶說願意付的一年 100 美元。",
        "zh-Hans": "比较了每月 8、10、12 美元。定在 10 美元时，年缴打八折是 96 美元，刚好低于客户说愿意付的一年 100 美元。"
      }
    },
    {
      "id": "ex-s-webhooks",
      "kind": "session",
      "agent": "codex",
      "space": "Launch",
      "day": 19,
      "title": {
        "en": "Coding session: payment webhooks",
        "zh-Hant": "寫程式紀錄：付款 webhook",
        "zh-Hans": "写程序记录：付款 webhook"
      },
      "body": {
        "en": "Webhook retries now back off for up to 24 hours. The provider requires an idempotency key on every refund request. Tests for duplicate refunds pass.",
        "zh-Hant": "Webhook 重試現在最多會延後到 24 小時。付款服務商要求每次退款都帶 idempotency key。重複退款的測試已經通過。",
        "zh-Hans": "Webhook 重试现在最多会延后到 24 小时。付款服务商要求每次退款都带 idempotency key。重复退款的测试已经通过。"
      }
    },
    {
      "id": "ex-p-terms",
      "kind": "pdf",
      "agent": "file",
      "space": "Launch",
      "day": 18,
      "title": {
        "en": "Payment provider terms.pdf",
        "zh-Hant": "付款服務商條款.pdf",
        "zh-Hans": "付款服务商条款.pdf"
      },
      "body": {
        "en": "New merchant accounts are reviewed for up to five business days before live payments are enabled. Every refund must reference the original charge.",
        "zh-Hant": "新商家帳號在開放正式收款前，最多會審核五個工作天。每筆退款都必須對應原本那筆收款。",
        "zh-Hans": "新商家账号在开放正式收款前，最多会审核五个工作日。每笔退款都必须对应原本那笔收款。"
      }
    },
    {
      "id": "ex-w-budgetly",
      "kind": "web",
      "agent": "webpage",
      "space": "Research",
      "day": 12,
      "title": {
        "en": "Budgetly pricing",
        "zh-Hant": "Budgetly 價格頁",
        "zh-Hans": "Budgetly 价格页"
      },
      "body": {
        "en": "Budgetly Pro costs $12 a month. There is no yearly plan. Bank sync is included.",
        "zh-Hant": "Budgetly Pro 每月 12 美元，沒有年繳方案，包含銀行同步。",
        "zh-Hans": "Budgetly Pro 每月 12 美元，没有年缴方案，包含银行同步。"
      }
    },
    {
      "id": "ex-i-ana",
      "kind": "interview",
      "agent": "file",
      "space": "Research",
      "day": 10,
      "title": {
        "en": "Interview: Ana, freelance designer",
        "zh-Hant": "訪談：Ana，接案設計師",
        "zh-Hans": "访谈：Ana，接案设计师"
      },
      "body": {
        "en": "I would pay $100 a year if I could send a clean export to my accountant. Setup took me twenty minutes and I almost gave up at the bank connection.",
        "zh-Hant": "如果能把整理好的資料直接匯出給會計師，我一年願意付 100 美元。設定花了我二十分鐘，在連接銀行那一步差點放棄。",
        "zh-Hans": "如果能把整理好的数据直接导出给会计师，我一年愿意付 100 美元。设置花了我二十分钟，在连接银行那一步差点放弃。"
      }
    },
    {
      "id": "ex-i-tom",
      "kind": "interview",
      "agent": "file",
      "space": "Research",
      "day": 11,
      "title": {
        "en": "Interview: Tom, photographer",
        "zh-Hant": "訪談：Tom，攝影師",
        "zh-Hans": "访谈：Tom，摄影师"
      },
      "body": {
        "en": "I connect my bank first thing, and that is where it broke for me. I mostly need a CSV I can hand to my accountant at tax time.",
        "zh-Hant": "我第一件事就是連接銀行，結果就是在那裡卡住。我最需要的是報稅時可以交給會計師的 CSV。",
        "zh-Hans": "我第一件事就是连接银行，结果就是在那里卡住。我最需要的是报税时可以交给会计师的 CSV。"
      }
    },
    {
      "id": "ex-c-faq",
      "kind": "chat",
      "agent": "chatgpt",
      "space": "Research",
      "day": 22,
      "title": {
        "en": "ChatGPT: support answers for v2",
        "zh-Hant": "ChatGPT：v2 客服回答",
        "zh-Hans": "ChatGPT：v2 客服回答"
      },
      "body": {
        "en": "Draft answers: the yearly plan can be switched to monthly at renewal. Refunds go back to the original card within ten days. If the bank connection fails, try again from Settings, then contact support.",
        "zh-Hant": "回答草稿：年繳方案可以在續約時改成月繳。退款會在十天內退回原本的卡。如果連接銀行失敗，先到設定裡重試，再聯絡客服。",
        "zh-Hans": "回答草稿：年缴方案可以在续约时改成月缴。退款会在十天内退回原本的卡。如果连接银行失败，先到设置里重试，再联系客服。"
      }
    },
    {
      "id": "ex-m-team",
      "kind": "notes",
      "agent": "obsidian",
      "space": "Research",
      "day": 3,
      "title": {
        "en": "People I work with",
        "zh-Hant": "一起工作的人",
        "zh-Hans": "一起工作的人"
      },
      "body": {
        "en": "Me, Maya Chen: product, running the launch. Pam Okafor, payments. Dev Patel, release notes and the website. Jonas Weber, backend. Sofia Rossi, support.",
        "zh-Hant": "我，Maya Chen：產品，負責整個上線。Pam Okafor，付款。Dev Patel，版本說明和網站。Jonas Weber，後端。Sofia Rossi，客服。",
        "zh-Hans": "我，Maya Chen：产品，负责整个上线。Pam Okafor，付款。Dev Patel，版本说明和网站。Jonas Weber，后端。Sofia Rossi，客服。"
      }
    }
  ],
  "pages": [
    {
      "id": "ex-page-launch",
      "space": "Launch",
      "title": {
        "en": "v2 launch plan",
        "zh-Hant": "v2 上線計畫",
        "zh-Hans": "v2 上线计划"
      },
      "sections": [
        {
          "heading": null,
          "sentences": [
            {
              "text": {
                "en": "Kestrel v2 launches on October 31.",
                "zh-Hant": "Kestrel v2 在 10 月 31 日上線。",
                "zh-Hans": "Kestrel v2 在 10 月 31 日上线。"
              },
              "sources": [
                "ex-m-sync-1021"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "It costs $10 a month, with 20% off the yearly plan.",
                "zh-Hant": "價格是每月 10 美元，年繳打八折。",
                "zh-Hans": "价格是每月 10 美元，年缴打八折。"
              },
              "sources": [
                "ex-m-sync-1021"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Pam owns the payment review and Dev owns the release notes.",
                "zh-Hant": "Pam 負責付款審核，Dev 負責版本說明。",
                "zh-Hans": "Pam 负责付款审核，Dev 负责版本说明。"
              },
              "sources": [
                "ex-m-sync-1021"
              ],
              "verified": true
            }
          ]
        },
        {
          "heading": {
            "en": "What changed",
            "zh-Hant": "後來改了什麼",
            "zh-Hans": "后来改了什么"
          },
          "sentences": [
            {
              "text": {
                "en": "Launch moved from October 24 to October 31, because the payment provider reviews new merchant accounts for up to five business days.",
                "zh-Hant": "上線從 10 月 24 日延到 10 月 31 日，因為付款服務商審核新商家帳號最多要五個工作天。",
                "zh-Hans": "上线从 10 月 24 日延到 10 月 31 日，因为付款服务商审核新商家账号最多要五个工作日。"
              },
              "sources": [
                "ex-m-sync-1014",
                "ex-p-terms"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "The price went from $8 to $10 a month after the customer interviews.",
                "zh-Hant": "客戶訪談之後，價格從每月 8 美元調到 10 美元。",
                "zh-Hans": "客户访谈之后，价格从每月 8 美元调到 10 美元。"
              },
              "sources": [
                "ex-m-sync-1014",
                "ex-c-pricing"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "We expect 400 paying customers in the first month.",
                "zh-Hant": "預計第一個月會有 400 個付費客戶。",
                "zh-Hans": "预计第一个月会有 400 个付费客户。"
              },
              "sources": [
                "ex-m-sync-1021"
              ],
              "verified": false
            }
          ]
        },
        {
          "heading": {
            "en": "See also",
            "zh-Hant": "另見",
            "zh-Hans": "另见"
          },
          "sentences": [
            {
              "text": {
                "en": "[[Pricing decision]], [[Payment review checklist]] and [[Release notes draft]].",
                "zh-Hant": "[[定價決定]]、[[付款審核清單]]和[[版本說明草稿]]。",
                "zh-Hans": "[[定价决定]]、[[付款审核清单]]和[[版本说明草稿]]。"
              },
              "sources": [],
              "verified": null
            }
          ]
        }
      ]
    },
    {
      "id": "ex-page-pricing",
      "space": "Launch",
      "title": {
        "en": "Pricing decision",
        "zh-Hant": "定價決定",
        "zh-Hans": "定价决定"
      },
      "sections": [
        {
          "heading": null,
          "sentences": [
            {
              "text": {
                "en": "Kestrel v2 costs $10 a month.",
                "zh-Hant": "Kestrel v2 每月 10 美元。",
                "zh-Hans": "Kestrel v2 每月 10 美元。"
              },
              "sources": [
                "ex-m-sync-1021"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "With 20% off, the yearly plan comes to $96, just under the $100 a year customers said they would pay.",
                "zh-Hant": "年繳打八折是 96 美元，剛好低於客戶說願意付的一年 100 美元。",
                "zh-Hans": "年缴打八折是 96 美元，刚好低于客户说愿意付的一年 100 美元。"
              },
              "sources": [
                "ex-c-pricing",
                "ex-i-ana"
              ],
              "verified": true
            }
          ]
        },
        {
          "heading": {
            "en": "Why not $12",
            "zh-Hant": "為什麼不是 12 美元",
            "zh-Hans": "为什么不是 12 美元"
          },
          "sentences": [
            {
              "text": {
                "en": "Budgetly Pro already costs $12 a month and has no yearly plan.",
                "zh-Hant": "Budgetly Pro 已經是每月 12 美元，而且沒有年繳方案。",
                "zh-Hans": "Budgetly Pro 已经是每月 12 美元，而且没有年缴方案。"
              },
              "sources": [
                "ex-w-budgetly"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Customers asked for an export to their accountant more than for a lower price.",
                "zh-Hant": "比起更低的價格，客戶更想要能匯出給會計師。",
                "zh-Hans": "比起更低的价格，客户更想要能导出给会计师。"
              },
              "sources": [
                "ex-i-ana",
                "ex-i-tom"
              ],
              "verified": true
            }
          ]
        }
      ]
    },
    {
      "id": "ex-page-payments",
      "space": "Launch",
      "title": {
        "en": "Payment review checklist",
        "zh-Hant": "付款審核清單",
        "zh-Hans": "付款审核清单"
      },
      "sections": [
        {
          "heading": null,
          "sentences": [
            {
              "text": {
                "en": "The payment provider reviews new merchant accounts for up to five business days before live payments.",
                "zh-Hant": "付款服務商在開放正式收款前，最多會審核新商家帳號五個工作天。",
                "zh-Hans": "付款服务商在开放正式收款前，最多会审核新商家账号五个工作日。"
              },
              "sources": [
                "ex-p-terms"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Every refund must reference the original charge.",
                "zh-Hant": "每筆退款都必須對應原本那筆收款。",
                "zh-Hans": "每笔退款都必须对应原本那笔收款。"
              },
              "sources": [
                "ex-p-terms"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Every refund request carries an idempotency key.",
                "zh-Hant": "每次退款請求都帶 idempotency key。",
                "zh-Hans": "每次退款请求都带 idempotency key。"
              },
              "sources": [
                "ex-s-webhooks"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Webhook retries back off for up to 24 hours.",
                "zh-Hant": "Webhook 重試最多延後到 24 小時。",
                "zh-Hans": "Webhook 重试最多延后到 24 小时。"
              },
              "sources": [
                "ex-s-webhooks"
              ],
              "verified": true
            }
          ]
        },
        {
          "heading": {
            "en": "Owner",
            "zh-Hant": "負責人",
            "zh-Hans": "负责人"
          },
          "sentences": [
            {
              "text": {
                "en": "Pam owns the payment review.",
                "zh-Hant": "Pam 負責付款審核。",
                "zh-Hans": "Pam 负责付款审核。"
              },
              "sources": [
                "ex-m-sync-1021"
              ],
              "verified": true
            }
          ]
        }
      ]
    },
    {
      "id": "ex-page-release",
      "space": "Launch",
      "title": {
        "en": "Release notes draft",
        "zh-Hant": "版本說明草稿",
        "zh-Hans": "版本说明草稿"
      },
      "sections": [
        {
          "heading": null,
          "sentences": [
            {
              "text": {
                "en": "New: a yearly plan at $96, 20% off the monthly price.",
                "zh-Hant": "新增：年繳方案 96 美元，比月繳便宜兩成。",
                "zh-Hans": "新增：年缴方案 96 美元，比月缴便宜两成。"
              },
              "sources": [
                "ex-c-pricing"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "New: export a CSV for your accountant.",
                "zh-Hant": "新增：匯出給會計師用的 CSV。",
                "zh-Hans": "新增：导出给会计师用的 CSV。"
              },
              "sources": [
                "ex-i-tom"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Better: connecting your bank takes fewer steps.",
                "zh-Hant": "改善：連接銀行的步驟變少了。",
                "zh-Hans": "改善：连接银行的步骤变少了。"
              },
              "sources": [
                "ex-m-retro"
              ],
              "verified": false
            }
          ]
        },
        {
          "heading": {
            "en": "Owner",
            "zh-Hant": "負責人",
            "zh-Hans": "负责人"
          },
          "sentences": [
            {
              "text": {
                "en": "Dev owns the release notes.",
                "zh-Hant": "Dev 負責版本說明。",
                "zh-Hans": "Dev 负责版本说明。"
              },
              "sources": [
                "ex-m-sync-1021"
              ],
              "verified": true
            }
          ]
        }
      ]
    },
    {
      "id": "ex-page-decisions",
      "space": "Launch",
      "title": {
        "en": "Decisions log",
        "zh-Hant": "決定紀錄",
        "zh-Hans": "决定记录"
      },
      "sections": [
        {
          "heading": {
            "en": "October 14",
            "zh-Hant": "10 月 14 日",
            "zh-Hans": "10 月 14 日"
          },
          "sentences": [
            {
              "text": {
                "en": "Launch set for October 24; the price stays at $8 until the interviews are in.",
                "zh-Hant": "上線定在 10 月 24 日；訪談結果出來前價格維持 8 美元。",
                "zh-Hans": "上线定在 10 月 24 日；访谈结果出来前价格维持 8 美元。"
              },
              "sources": [
                "ex-m-sync-1014"
              ],
              "verified": true
            }
          ]
        },
        {
          "heading": {
            "en": "October 21",
            "zh-Hant": "10 月 21 日",
            "zh-Hans": "10 月 21 日"
          },
          "sentences": [
            {
              "text": {
                "en": "Launch moved to October 31 for the payment review.",
                "zh-Hant": "因為付款審核，上線改到 10 月 31 日。",
                "zh-Hans": "因为付款审核，上线改到 10 月 31 日。"
              },
              "sources": [
                "ex-m-sync-1021"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Price set at $10 a month, 20% off yearly.",
                "zh-Hant": "價格定為每月 10 美元，年繳打八折。",
                "zh-Hans": "价格定为每月 10 美元，年缴打八折。"
              },
              "sources": [
                "ex-m-sync-1021"
              ],
              "verified": true
            }
          ]
        }
      ]
    },
    {
      "id": "ex-page-interviews",
      "space": "Research",
      "title": {
        "en": "Customer interviews: onboarding",
        "zh-Hant": "客戶訪談：新手設定",
        "zh-Hans": "客户访谈：新手设置"
      },
      "sections": [
        {
          "heading": null,
          "sentences": [
            {
              "text": {
                "en": "Half of new sign-ups never finish setup, and most stop at the bank connection step.",
                "zh-Hant": "一半的新註冊用戶沒有完成設定，大多停在連接銀行那一步。",
                "zh-Hans": "一半的新注册用户没有完成设置，大多停在连接银行那一步。"
              },
              "sources": [
                "ex-m-retro"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Ana took twenty minutes to set up and almost gave up at the bank connection.",
                "zh-Hant": "Ana 設定花了二十分鐘，在連接銀行那一步差點放棄。",
                "zh-Hans": "Ana 设置花了二十分钟，在连接银行那一步差点放弃。"
              },
              "sources": [
                "ex-i-ana"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Tom mostly needs a CSV he can hand to his accountant at tax time.",
                "zh-Hant": "Tom 最需要的是報稅時可以交給會計師的 CSV。",
                "zh-Hans": "Tom 最需要的是报税时可以交给会计师的 CSV。"
              },
              "sources": [
                "ex-i-tom"
              ],
              "verified": true
            }
          ]
        }
      ]
    },
    {
      "id": "ex-page-competitors",
      "space": "Research",
      "title": {
        "en": "Competitor notes",
        "zh-Hant": "競品筆記",
        "zh-Hans": "竞品笔记"
      },
      "sections": [
        {
          "heading": null,
          "sentences": [
            {
              "text": {
                "en": "Budgetly Pro costs $12 a month and includes bank sync.",
                "zh-Hant": "Budgetly Pro 每月 12 美元，包含銀行同步。",
                "zh-Hans": "Budgetly Pro 每月 12 美元，包含银行同步。"
              },
              "sources": [
                "ex-w-budgetly"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Budgetly has no yearly plan.",
                "zh-Hant": "Budgetly 沒有年繳方案。",
                "zh-Hans": "Budgetly 没有年缴方案。"
              },
              "sources": [
                "ex-w-budgetly"
              ],
              "verified": true
            }
          ]
        }
      ]
    },
    {
      "id": "ex-page-faq",
      "space": "Research",
      "title": {
        "en": "Support FAQ for v2",
        "zh-Hant": "v2 客服常見問題",
        "zh-Hans": "v2 客服常见问题"
      },
      "sections": [
        {
          "heading": null,
          "sentences": [
            {
              "text": {
                "en": "The yearly plan can be switched to monthly at renewal.",
                "zh-Hant": "年繳方案可以在續約時改成月繳。",
                "zh-Hans": "年缴方案可以在续约时改成月缴。"
              },
              "sources": [
                "ex-c-faq"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Refunds go back to the original card within ten days.",
                "zh-Hant": "退款會在十天內退回原本的卡。",
                "zh-Hans": "退款会在十天内退回原本的卡。"
              },
              "sources": [
                "ex-c-faq"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "If the bank connection fails, try again from Settings, then contact support.",
                "zh-Hant": "如果連接銀行失敗，先到設定裡重試，再聯絡客服。",
                "zh-Hans": "如果连接银行失败，先到设置里重试，再联系客服。"
              },
              "sources": [
                "ex-c-faq"
              ],
              "verified": true
            }
          ]
        }
      ]
    },
    {
      "id": "ex-page-people",
      "space": "Research",
      "title": {
        "en": "Who's who",
        "zh-Hant": "誰負責什麼",
        "zh-Hans": "谁负责什么"
      },
      "sections": [
        {
          "heading": null,
          "sentences": [
            {
              "text": {
                "en": "Maya Chen runs the launch.",
                "zh-Hant": "Maya Chen 負責整個上線。",
                "zh-Hans": "Maya Chen 负责整个上线。"
              },
              "sources": [
                "ex-m-team"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Pam Okafor handles payments, and Dev Patel the release notes and the website.",
                "zh-Hant": "Pam Okafor 負責付款，Dev Patel 負責版本說明和網站。",
                "zh-Hans": "Pam Okafor 负责付款，Dev Patel 负责版本说明和网站。"
              },
              "sources": [
                "ex-m-team"
              ],
              "verified": true
            },
            {
              "text": {
                "en": "Jonas Weber works on the backend, and Sofia Rossi on support.",
                "zh-Hant": "Jonas Weber 做後端，Sofia Rossi 做客服。",
                "zh-Hans": "Jonas Weber 做后端，Sofia Rossi 做客服。"
              },
              "sources": [
                "ex-m-team"
              ],
              "verified": true
            }
          ]
        }
      ]
    }
  ],
  "spaces": [
    {
      "id": "Launch",
      "name": {
        "en": "Launch",
        "zh-Hant": "上線",
        "zh-Hans": "上线"
      },
      "description": {
        "en": "Everything for getting v2 out the door.",
        "zh-Hant": "把 v2 上線要做的事都放這裡。",
        "zh-Hans": "把 v2 上线要做的事都放这里。"
      }
    },
    {
      "id": "Research",
      "name": {
        "en": "Research",
        "zh-Hant": "研究",
        "zh-Hans": "研究"
      },
      "description": {
        "en": "What customers and competitors said.",
        "zh-Hant": "客戶和競品怎麼說。",
        "zh-Hans": "客户和竞品怎么说。"
      }
    }
  ],
  "entities": [
    {
      "id": "ex-e-maya",
      "type": "person",
      "name": "Maya Chen",
      "pages": [
        "ex-page-people",
        "ex-page-launch"
      ]
    },
    {
      "id": "ex-e-pam",
      "type": "person",
      "name": "Pam Okafor",
      "pages": [
        "ex-page-people",
        "ex-page-payments",
        "ex-page-launch"
      ]
    },
    {
      "id": "ex-e-dev",
      "type": "person",
      "name": "Dev Patel",
      "pages": [
        "ex-page-people",
        "ex-page-release",
        "ex-page-launch"
      ]
    },
    {
      "id": "ex-e-kestrel",
      "type": "project",
      "name": "Kestrel v2",
      "pages": [
        "ex-page-launch",
        "ex-page-pricing",
        "ex-page-release"
      ]
    },
    {
      "id": "ex-e-budgetly",
      "type": "organization",
      "name": "Budgetly",
      "pages": [
        "ex-page-competitors",
        "ex-page-pricing"
      ]
    }
  ]
};
