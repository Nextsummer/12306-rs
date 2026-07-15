use std::{net::SocketAddr, path::PathBuf, sync::OnceLock, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Html,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::NaiveDate;
use rs12306_client_12306::{LoginRequest, LoginResult, login_12306};
use rs12306_core::{
    DEFAULT_QUERY_INTERVAL_MS, MIN_QUERY_INTERVAL_MS, NewTicketTask, NewTrainPolicy, Passenger,
    PassengerId, PassengerType, SeatType, Station, TaskStatus, TrainFilter, TrainFilterKind,
};
use rs12306_storage::{
    DEFAULT_DATABASE_PATH, Database, NewTrainRecord, StorageError, TaskDetails, TaskLog,
    TaskSummary,
};
use serde::{Deserialize, Serialize};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub database_path: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 12306,
            database_path: PathBuf::from(DEFAULT_DATABASE_PATH),
        }
    }
}

impl ServerConfig {
    pub fn socket_addr(&self) -> anyhow::Result<SocketAddr> {
        Ok(format!("{}:{}", self.host, self.port).parse()?)
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    database: Database,
    config: ServerConfig,
}

pub fn router(database: Database, config: ServerConfig) -> Router {
    let state = AppState { database, config };

    Router::new()
        .route("/", get(page_dashboard))
        .route("/tickets", get(page_tickets))
        .route("/tasks/new", get(page_task_new))
        .route("/tasks", get(page_tasks))
        .route("/tasks/{task_id}", get(page_task_detail))
        .route("/login", get(page_login))
        .route("/settings", get(page_settings))
        .route("/api/health", get(health))
        .route("/api/settings", get(settings).put(update_settings))
        .route("/api/session/status", get(session_status))
        .route("/api/session/login", post(session_login))
        .route("/api/session/logout", post(session_logout))
        .route("/api/passengers", get(list_passengers).post(save_passenger))
        .route(
            "/api/session/verification/open",
            post(session_verification_open),
        )
        .route(
            "/api/session/verification/complete",
            post(session_verification_complete),
        )
        .route("/api/tickets/query", get(query_tickets))
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/{task_id}", get(get_task))
        .route("/api/tasks/{task_id}/start", post(start_task))
        .route("/api/tasks/{task_id}/pause", post(pause_task))
        .route("/api/tasks/{task_id}/resume", post(resume_task))
        .route("/api/tasks/{task_id}/cancel", post(cancel_task))
        .route("/api/tasks/{task_id}/logs", get(list_task_logs))
        .route("/api/tasks/{task_id}/new-trains", get(list_new_trains))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn serve(config: ServerConfig) -> anyhow::Result<()> {
    let database = Database::open(&config.database_path)?;
    let addr = config.socket_addr()?;
    let app = router(database, config);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("12306-rs server listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn page_dashboard() -> Html<String> {
    render_page(include_str!("../static/dashboard.html"))
}

async fn page_tickets() -> Html<String> {
    render_page(include_str!("../static/tickets.html"))
}

async fn page_task_new() -> Html<String> {
    render_page(include_str!("../static/task_new.html"))
}

async fn page_tasks() -> Html<String> {
    render_page(include_str!("../static/tasks.html"))
}

async fn page_task_detail(Path(_task_id): Path<String>) -> Html<String> {
    render_page(include_str!("../static/task_detail.html"))
}

async fn page_login() -> Html<String> {
    render_page(include_str!("../static/login.html"))
}

async fn page_settings() -> Html<String> {
    render_page(include_str!("../static/settings.html"))
}

fn render_page(html: &'static str) -> Html<String> {
    Html(html.replace("</body>", UI_BOOTSTRAP_SCRIPT))
}

const UI_BOOTSTRAP_SCRIPT: &str = r#"
<script>
(() => {
  const routes = [
    ["总览仪表盘", "/"],
    ["控制面板", "/"],
    ["车票查询", "/tickets"],
    ["创建任务", "/tasks/new"],
    ["创建抢票任务", "/tasks/new"],
    ["任务管理", "/tasks"],
    ["任务列表", "/tasks"],
    ["系统设置", "/settings"]
  ];
  const official12306Url = "https://kyfw.12306.cn/otn/view/index.html";

  function normalize(text) {
    return (text || "").replace(/\s+/g, "");
  }

  document.addEventListener("click", (event) => {
    const target = event.target.closest("button, a");
    if (!target) return;
    const text = normalize(target.textContent);
    if (text.includes("前往支付") || text.includes("去支付")) {
      event.preventDefault();
      window.open(official12306Url, "_blank", "noopener");
    }
  });

  document.querySelectorAll("a").forEach((anchor) => {
    const text = normalize(anchor.textContent);
    const route = routes.find(([label]) => text.includes(label));
    if (route) anchor.href = route[1];
  });

  document.querySelectorAll("button").forEach((button) => {
    const text = normalize(button.textContent);
    if (text.includes("新建任务") || text.includes("创建抢票任务")) {
      button.addEventListener("click", () => { window.location.href = "/tasks/new"; });
    }
    if (window.location.pathname !== "/login" && (text.includes("重新登录") || text.includes("去验证") || text.includes("立即处理"))) {
      button.addEventListener("click", () => { window.location.href = "/login"; });
    }
    if (text.includes("查看详情")) {
      button.addEventListener("click", () => { window.location.href = "/tasks/demo"; });
    }
  });

  const passengerSeed = {
    "1": "00000000-0000-4000-8000-000000000001",
    "2": "00000000-0000-4000-8000-000000000002",
    "3": "00000000-0000-4000-8000-000000000003"
  };

  const passengerTypeLabelMap = {
    adult: "成人票",
    child: "儿童票",
    student: "学生票",
    disabled_military: "残军票"
  };

  const seatNameMap = {
    "商务座": "business",
    "一等座": "first_class",
    "二等座": "second_class",
    "软卧": "soft_sleeper",
    "硬卧": "hard_sleeper",
    "硬座": "hard_seat",
    "无座": "no_seat"
  };

  const seatLabelMap = {
    business: "商务座",
    first_class: "一等座",
    second_class: "二等座",
    soft_sleeper: "软卧",
    hard_sleeper: "硬卧",
    hard_seat: "硬座",
    no_seat: "无座"
  };

  const stationNameMap = {
    BJP: "北京",
    VNP: "北京南",
    SHH: "上海",
    AOH: "上海虹桥",
    IZQ: "广州南",
    IOQ: "深圳北"
  };

  const statusMeta = {
    created: ["new_releases", "已创建", "bg-secondary/10 border-secondary/20 text-secondary"],
    running: ["play_circle", "运行中", "bg-primary/10 border-primary/30 text-primary"],
    querying: ["search", "查询中", "bg-primary/10 border-primary/20 text-primary"],
    waiting_login: ["login", "等待登录", "bg-error/10 border-error/30 text-error"],
    verification_required: ["verified_user", "等待人工验证", "bg-error/10 border-error/30 text-error"],
    paused: ["pause_circle", "已暂停", "bg-surface-variant/40 border-outline-variant text-on-surface-variant"],
    submitting: ["sync", "正在提交", "bg-secondary/10 border-secondary/20 text-secondary"],
    pending_payment: ["payments", "普通订单待支付", "bg-tertiary/20 border-tertiary/40 text-tertiary"],
    candidate_submitting: ["hourglass_top", "正在提交候补", "bg-secondary/10 border-secondary/20 text-secondary"],
    candidate_submitted: ["hourglass_empty", "候补已提交等待兑现", "bg-secondary/20 border-secondary/40 text-secondary"],
    candidate_pending_payment: ["confirmation_number", "候补兑现成功待支付", "bg-primary/20 border-primary/40 text-primary"],
    failed: ["report", "失败", "bg-error/20 border-error/40 text-error"],
    cancelled: ["stop_circle", "已取消", "bg-surface-variant/40 border-outline-variant text-on-surface-variant"]
  };

  function escapeHtml(value) {
    return String(value ?? "").replace(/[&<>"']/g, (char) => ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;"
    })[char]);
  }

  function statusBadge(status) {
    const [icon, label, classes] = statusMeta[status] || ["help", status, "bg-surface-variant/40 border-outline-variant text-on-surface-variant"];
    return `<div class="inline-flex items-center gap-2 px-2.5 py-1 rounded-md border ${classes}">
      <span class="material-symbols-outlined text-[14px]">${icon}</span>
      <span class="text-xs font-bold">${escapeHtml(label)}</span>
    </div>`;
  }

  function actionButton(taskId, action, icon, title) {
    return `<button data-task-action="${action}" data-task-id="${escapeHtml(taskId)}" class="w-8 h-8 rounded-full flex items-center justify-center text-on-surface-variant hover:text-primary hover:bg-primary/10 transition-colors tooltip" title="${title}">
      <span class="material-symbols-outlined text-[18px]">${icon}</span>
    </button>`;
  }

  async function fetchJson(url, options) {
    const response = await fetch(url, options);
    if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    return response.json();
  }

  async function loadTaskDetails(id) {
    return fetchJson(`/api/tasks/${encodeURIComponent(id)}`);
  }

  function seatLabel(seat) {
    return seatLabelMap[seat] || seat || "-";
  }

  function trainFilterLabel(task) {
    if (task.train_include?.length) return task.train_include.join(", ");
    if (task.train_exclude?.length) return `排除 ${task.train_exclude.join(", ")}`;
    return "所有列车";
  }

  function newTrainPolicyLabel(task) {
    const action = {
      off: "关闭",
      notify_only: "仅通知",
      auto_order: "通知并下单"
    }[task.new_train_policy] || task.new_train_policy;
    return task.new_trains_only ? `仅新增 · ${action}` : `普通任务 · ${action}`;
  }

  function taskActionButtons(task) {
    const buttons = [];
    if (["pending_payment", "candidate_pending_payment"].includes(task.status)) {
      buttons.push(`<button class="px-3 py-1.5 rounded-lg bg-tertiary text-on-tertiary text-xs font-bold hover:scale-105 transition-transform shadow-[0_0_10px_rgba(200,160,240,0.2)]">去支付</button>`);
    }
    if (task.status === "created") buttons.push(actionButton(task.id, "start", "play_arrow", "启动"));
    if (["running", "querying"].includes(task.status)) buttons.push(actionButton(task.id, "pause", "pause", "暂停"));
    if (["paused", "waiting_login", "verification_required"].includes(task.status)) buttons.push(actionButton(task.id, "resume", "replay", "恢复"));
    if (!["failed", "cancelled"].includes(task.status)) buttons.push(actionButton(task.id, "cancel", "stop", "取消"));
    return buttons.join("");
  }

  function parseStation(value, fallbackName, fallbackCode) {
    const match = String(value || "").match(/^\s*(.*?)\s*[（(]([A-Z0-9]+)[）)]\s*$/i);
    if (!match) return { name: fallbackName, code: fallbackCode };
    return { name: match[1].trim() || fallbackName, code: match[2].trim().toUpperCase() || fallbackCode };
  }

  function swapInputValues(inputs) {
    if (inputs.length < 2) return;
    [inputs[0].value, inputs[1].value] = [inputs[1].value, inputs[0].value];
  }

  function stationInputLabel(code) {
    const normalizedCode = String(code || "").trim().toUpperCase();
    const name = stationNameMap[normalizedCode] || normalizedCode;
    return `${name} (${normalizedCode})`;
  }

  function replaceExactText(from, to) {
    document.querySelectorAll("span, div, p, li").forEach((node) => {
      if (normalize(node.textContent) === normalize(from)) node.textContent = to;
    });
  }

  function setFirstTextNode(node, value) {
    if (!node) return;
    const textNode = Array.from(node.childNodes).find((child) => child.nodeType === Node.TEXT_NODE);
    if (textNode) {
      textNode.textContent = value;
    } else {
      node.textContent = value;
    }
  }

  function applyCreateTaskParams() {
    const params = new URLSearchParams(window.location.search);
    const from = params.get("from");
    const to = params.get("to");
    const date = params.get("date");
    const train = params.get("train");
    if (!from && !to && !date && !train) return;

    const stationInputs = Array.from(document.querySelectorAll("input[placeholder='输入城市或车站拼音']"));
    if (from && stationInputs[0]) {
      const label = stationInputLabel(from);
      stationInputs[0].value = label;
      replaceExactText("北京 (BJP)", label);
    }
    if (to && stationInputs[1]) {
      const label = stationInputLabel(to);
      stationInputs[1].value = label;
      replaceExactText("上海 (SHH)", label);
    }
    if (date) {
      const dateNodes = Array.from(document.querySelectorAll("span"))
        .filter((node) => /\d{4}-\d{2}-\d{2}/.test(node.textContent));
      setFirstTextNode(dateNodes[0], `${date} `);
      dateNodes.slice(1).forEach((node) => node.remove());
      replaceExactText("2026-07-05, 2026-07-06", date);
    }
    if (train) {
      const trainNodes = Array.from(document.querySelectorAll("span.font-mono"))
        .filter((node) => /^[A-Z]\d+/i.test(normalize(node.textContent)));
      setFirstTextNode(trainNodes[0], ` ${train.toUpperCase()} `);
      trainNodes.slice(1).forEach((node) => node.remove());
      replaceExactText("限定车次 (2个)", "限定车次 (1个)");
    }
  }

  function readTaskDraftFromPrototype() {
    const stationInputs = Array.from(document.querySelectorAll("input[placeholder='输入城市或车站拼音']"));
    const from = parseStation(stationInputs[0]?.value, "北京", "BJP");
    const to = parseStation(stationInputs[1]?.value, "上海", "SHH");
    const dates = Array.from(document.querySelectorAll("span"))
      .map((node) => node.textContent.match(/\d{4}-\d{2}-\d{2}/)?.[0])
      .filter(Boolean);
    const passengerIds = Array.from(document.querySelectorAll("input[name='passenger']:checked"))
      .map((input) => /^[0-9a-f-]{36}$/i.test(input.value) ? input.value : passengerSeed[input.value])
      .filter(Boolean);
    const seatPreferences = Array.from(document.querySelectorAll("section"))
      .find((section) => normalize(section.textContent).includes("席别优先级"))
      ? Array.from(document.querySelectorAll("div.flex.items-center.space-x-2 span.text-sm"))
          .map((node) => seatNameMap[normalize(node.textContent)])
          .filter(Boolean)
      : ["second_class"];
    const trainFilterType = document.querySelector("input[name='train_filter_type']:checked")?.value || "include";
    const trainTokens = Array.from(document.querySelectorAll("span.font-mono"))
      .map((node) => normalize(node.childNodes[0]?.textContent || node.textContent))
      .filter((value) => /^[A-Z]\d+$/i.test(value));
    const queryInterval = Number(document.querySelector("input[type='number']")?.value || 3000);
    const payload = {
      from_name: from.name,
      from_code: from.code,
      to_name: to.name,
      to_code: to.code,
      dates: [...new Set(dates.length ? dates : ["2026-07-10"])],
      passenger_ids: passengerIds,
      seat_preferences: seatPreferences.length ? [...new Set(seatPreferences)] : ["second_class"],
      accept_no_seat: Boolean(document.getElementById("no-seat-toggle-new")?.checked),
      train_include: trainFilterType === "include" ? trainTokens : [],
      train_exclude: trainFilterType === "exclude" ? trainTokens : [],
      enable_waitlist: Boolean(document.getElementById("enable-waitlist")?.checked),
      enable_strong_waitlist: Boolean(document.getElementById("strong-waitlist")?.checked),
      new_train_policy: document.getElementById("new-train-policy")?.value || "off",
      new_trains_only: Boolean(document.getElementById("new-trains-only")?.checked),
      query_interval_ms: Math.max(1000, queryInterval),
      remark: null
    };
    if (payload.enable_strong_waitlist) payload.enable_waitlist = true;
    if (payload.new_trains_only && payload.new_train_policy === "notify_only") {
      payload.enable_waitlist = false;
      payload.enable_strong_waitlist = false;
    }
    return payload;
  }

  async function createTaskFromPrototype(startImmediately) {
    const payload = readTaskDraftFromPrototype();
    const task = await fetchJson("/api/tasks", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(payload)
    });
    if (startImmediately) {
      try {
        await fetchJson(`/api/tasks/${task.id}/start`, { method: "POST" });
      } catch (_) {
        // Keep the created task if starting is not allowed yet.
      }
    }
    window.location.href = `/tasks/${task.id}`;
  }

  function addPillBeforeInput(input, html) {
    input.insertAdjacentHTML("beforebegin", html);
    input.value = "";
  }

  function wirePillRemoval(input) {
    input?.parentElement?.addEventListener("click", (event) => {
      if (normalize(event.target.textContent) !== "close") return;
      event.preventDefault();
      event.target.closest("span.inline-flex")?.remove();
    });
  }

  function renderPassengerOption(passenger, index) {
    const checked = index < 2 ? "checked" : "";
    const active = index < 2;
    return `<label class="relative flex cursor-pointer rounded-lg border ${active ? "border-primary bg-secondary-container/30" : "border-outline-variant bg-surface-container-highest hover:bg-surface-container-high"} p-3 shadow-sm transition-colors focus:outline-none">
      <input ${checked} class="sr-only" name="passenger" type="checkbox" value="${escapeHtml(passenger.id)}"/>
      <div class="flex w-full items-center justify-between">
        <div class="flex items-center"><div class="text-sm">
          <p class="font-medium ${active ? "text-on-surface" : "text-on-surface-variant"}">${escapeHtml(passenger.name)}</p>
          <p class="text-xs text-on-surface-variant mt-0.5">${escapeHtml(passengerTypeLabelMap[passenger.passenger_type] || passenger.passenger_type)} · ${escapeHtml(passenger.id_masked)}</p>
        </div></div>
        <span class="material-symbols-outlined ${active ? "text-primary" : "text-outline-variant"} text-xl">${active ? "check_circle" : "circle"}</span>
      </div>
    </label>`;
  }

  function syncPassengerCount() {
    const label = Array.from(document.querySelectorAll("label")).find((node) => normalize(node.textContent).startsWith("选择乘车人"));
    if (!label) return;
    const inputs = Array.from(document.querySelectorAll("input[name='passenger']"));
    inputs.forEach((input) => {
      const card = input.closest("label");
      const name = card?.querySelector("p.font-medium");
      const icon = card?.querySelector(".material-symbols-outlined");
      const active = input.checked;
      card?.classList.toggle("border-primary", active);
      card?.classList.toggle("bg-secondary-container/30", active);
      card?.classList.toggle("border-outline-variant", !active);
      card?.classList.toggle("bg-surface-container-highest", !active);
      name?.classList.toggle("text-on-surface", active);
      name?.classList.toggle("text-on-surface-variant", !active);
      if (icon) {
        icon.textContent = active ? "check_circle" : "circle";
        icon.classList.toggle("text-primary", active);
        icon.classList.toggle("text-outline-variant", !active);
      }
    });
    const total = inputs.length;
    const selected = inputs.filter((input) => input.checked).length;
    label.textContent = `选择乘车人 (已选 ${selected}/${total})`;
  }

  async function hydrateCreatePassengers() {
    const passengers = await fetchJson("/api/passengers");
    if (!passengers.length) return;
    const grid = document.querySelector("input[name='passenger']")?.closest(".grid");
    if (!grid) return;
    grid.innerHTML = passengers.map(renderPassengerOption).join("");
    grid.querySelectorAll("input[name='passenger']").forEach((input) => {
      input.addEventListener("change", syncPassengerCount);
    });
    syncPassengerCount();
  }

  async function hydrateCreateTaskPage() {
    if (window.location.pathname !== "/tasks/new") return;
    applyCreateTaskParams();
    await hydrateCreatePassengers();
    Array.from(document.querySelectorAll("button"))
      .find((button) => normalize(button.textContent).includes("sync_alt"))
      ?.addEventListener("click", () => swapInputValues(Array.from(document.querySelectorAll("input[placeholder='输入城市或车站拼音']"))));
    const intervalNumber = document.querySelector("input[type='number']");
    const intervalRange = document.querySelector("input[type='range']");
    const syncInterval = (value) => {
      const next = String(Math.min(5000, Math.max(1000, Number(value) || 3000)));
      if (intervalNumber) intervalNumber.value = next;
      if (intervalRange) intervalRange.value = next;
    };
    intervalNumber?.addEventListener("change", () => syncInterval(intervalNumber.value));
    intervalRange?.addEventListener("input", () => syncInterval(intervalRange.value));
    const dateInput = document.querySelector("input[placeholder='选择日期...']");
    wirePillRemoval(dateInput);
    const clearDates = Array.from(document.querySelectorAll("span")).find((node) => normalize(node.textContent) === "清空选择");
    clearDates?.addEventListener("click", () => {
      dateInput?.parentElement?.querySelectorAll("span.inline-flex").forEach((node) => node.remove());
    });
    dateInput?.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      const date = dateInput.value.trim();
      if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) return;
      addPillBeforeInput(dateInput, `<span class="inline-flex items-center px-2.5 py-1 rounded bg-secondary-container text-on-primary-container text-sm border border-primary-container">${escapeHtml(date)} <button class="ml-1.5 text-on-primary-container/70 hover:text-on-primary-container"><span class="material-symbols-outlined text-sm block">close</span></button></span>`);
    });
    const trainInput = document.querySelector("input[placeholder^='输入车次']");
    wirePillRemoval(trainInput);
    trainInput?.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      const train = trainInput.value.trim().toUpperCase();
      if (!/^[A-Z]\d+$/i.test(train)) return;
      addPillBeforeInput(trainInput, `<span class="inline-flex items-center px-2.5 py-1 rounded bg-surface-container-low border border-outline text-on-surface text-sm font-mono"> ${escapeHtml(train)} <button class="ml-1.5 text-on-surface-variant hover:text-error"><span class="material-symbols-outlined text-sm block">close</span></button></span>`);
    });
    const waitlist = document.getElementById("enable-waitlist");
    const strongWaitlist = document.getElementById("strong-waitlist");
    const newTrainPolicy = document.getElementById("new-train-policy");
    const newTrainsOnly = document.getElementById("new-trains-only");
    const syncNewTrainControls = () => {
      if (newTrainsOnly?.checked && newTrainPolicy?.value === "off") {
        newTrainPolicy.value = "notify_only";
      }
      if (newTrainsOnly?.checked && newTrainPolicy?.value === "notify_only") {
        if (waitlist) waitlist.checked = false;
        if (strongWaitlist) strongWaitlist.checked = false;
      }
    };
    newTrainPolicy?.addEventListener("change", syncNewTrainControls);
    newTrainsOnly?.addEventListener("change", syncNewTrainControls);
    strongWaitlist?.addEventListener("change", () => {
      if (strongWaitlist.checked && waitlist) waitlist.checked = true;
    });
    waitlist?.addEventListener("change", () => {
      if (!waitlist.checked && strongWaitlist) strongWaitlist.checked = false;
    });
    const buttons = Array.from(document.querySelectorAll("button"));
    const createAndStart = buttons.find((button) => normalize(button.textContent).includes("创建并启动任务"));
    const saveDraft = buttons.find((button) => normalize(button.textContent).includes("仅保存草稿"));
    createAndStart?.addEventListener("click", (event) => {
      event.preventDefault();
      createTaskFromPrototype(true).catch((error) => alert(`创建任务失败: ${error.message}`));
    });
    saveDraft?.addEventListener("click", (event) => {
      event.preventDefault();
      createTaskFromPrototype(false).catch((error) => alert(`保存任务失败: ${error.message}`));
    });
  }

  function renderTaskRow(task) {
    const primaryDate = task.dates?.[0] || "-";
    const passengers = task.passenger_ids?.length ? `${task.passenger_ids.length} 位乘车人` : "-";
    const seats = task.seat_types?.length ? task.seat_types.map(seatLabel).join(", ") : "未设置席别";
    const trains = trainFilterLabel(task);
    const newTrainPolicy = newTrainPolicyLabel(task);
    return `<tr class="hover:bg-surface-variant/30 transition-colors group">
      <td class="px-6 py-4 text-on-surface font-mono text-xs">${escapeHtml(task.id.slice(0, 8))}</td>
      <td class="px-6 py-4"><div class="flex flex-col">
        <span class="font-medium text-on-surface flex items-center gap-2">${escapeHtml(task.from_name)} <span class="material-symbols-outlined text-[14px] text-primary/70">arrow_right_alt</span> ${escapeHtml(task.to_name)}</span>
        <span class="text-xs text-on-surface-variant mt-0.5">${escapeHtml(trains)} (${escapeHtml(seats)})</span>
        <span class="text-xs text-on-surface-variant mt-0.5">${escapeHtml(newTrainPolicy)}</span>
      </div></td>
      <td class="px-6 py-4 text-on-surface-variant">${escapeHtml(primaryDate)}</td>
      <td class="px-6 py-4 text-on-surface-variant">${escapeHtml(passengers)}</td>
      <td class="px-6 py-4">${statusBadge(task.status)}</td>
      <td class="px-6 py-4 text-right"><div class="flex items-center justify-end gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
        ${taskActionButtons(task)}
        <button data-task-detail="${escapeHtml(task.id)}" class="w-8 h-8 rounded-full flex items-center justify-center text-on-surface-variant hover:text-primary hover:bg-primary/10 transition-colors tooltip" title="详情">
          <span class="material-symbols-outlined text-[18px]">info</span>
        </button>
      </div></td>
    </tr>`;
  }

  async function hydrateTaskListPage() {
    if (window.location.pathname !== "/tasks") return;
    const tbody = document.querySelector("tbody");
    if (!tbody) return;
    const summaries = await fetchJson("/api/tasks");
    if (!summaries.length) {
      tbody.innerHTML = `<tr><td colspan="6" class="px-6 py-12 text-center text-on-surface-variant">暂无任务，点击右上角新建任务。</td></tr>`;
      return;
    }
    const details = await Promise.all(summaries.map((task) => loadTaskDetails(task.id)));
    tbody.innerHTML = details.map(renderTaskRow).join("");
    document.querySelectorAll("[data-task-detail]").forEach((button) => {
      button.addEventListener("click", () => { window.location.href = `/tasks/${button.dataset.taskDetail}`; });
    });
    document.querySelectorAll("[data-task-action]").forEach((button) => {
      button.addEventListener("click", async () => {
        await fetchJson(`/api/tasks/${button.dataset.taskId}/${button.dataset.taskAction}`, { method: "POST" });
        await hydrateTaskListPage();
      });
    });
  }

  function setConfigValue(labelPrefix, html) {
    const labels = Array.from(document.querySelectorAll("p")).filter((node) => normalize(node.textContent).startsWith(labelPrefix));
    const label = labels[0];
    if (!label || !label.parentElement) return;
    const target = Array.from(label.parentElement.children).find((child) => child !== label);
    if (target) target.innerHTML = html;
  }

  function renderToken(value) {
    return `<span class="px-2 py-0.5 rounded text-xs font-mono font-medium bg-primary/10 text-primary border border-primary/20">${escapeHtml(value)}</span>`;
  }

  function statusLabel(status) {
    return statusMeta[status]?.[1] || status;
  }

  function renderDashboardTaskRow(task) {
    const train = trainFilterLabel(task);
    const date = task.dates?.[0] || "-";
    const seat = seatLabel(task.seat_types?.[0] || (task.accept_no_seat ? "no_seat" : "-"));
    return `<tr class="hover:bg-surface-variant/40 transition-colors group">
      <td class="py-3 px-4"><div class="flex flex-col gap-1">
        <div class="flex items-center gap-2">
          <span class="font-medium text-primary text-sm">${escapeHtml(task.from_name)} - ${escapeHtml(task.to_name)}</span>
          <span class="bg-surface-variant px-1.5 py-0.5 rounded text-[10px] font-mono border border-outline-variant">${escapeHtml(train)}</span>
        </div>
        <span class="text-xs text-on-surface-variant">${escapeHtml(date)} · ${escapeHtml(seat)} · 频率 ${escapeHtml(task.query_interval_ms)}ms</span>
      </div></td>
      <td class="py-3 px-4">${statusBadge(task.status)}</td>
      <td class="py-3 px-4 text-right">
        <button data-task-detail="${escapeHtml(task.id)}" class="text-on-surface-variant hover:text-primary transition-colors p-1" title="查看详情">
          <span class="material-symbols-outlined text-[18px]">visibility</span>
        </button>
      </td>
    </tr>`;
  }

  function availabilityClass(value) {
    if (value === "有" || /^\d+$/.test(value)) return "text-primary font-medium";
    if (value === "--") return "text-on-surface-variant/30";
    return "text-on-surface-variant/50";
  }

  function renderAvailability(value) {
    return `<span class="${availabilityClass(value)}">${escapeHtml(value)}</span>`;
  }

  function renderTicketRow(ticket) {
    const theme = ticket.train_no.startsWith("D")
      ? {
          train: "font-semibold text-secondary group-hover:text-secondary-fixed-dim transition-colors",
          line: "h-[1px] w-full bg-secondary/20 absolute top-1/2 -translate-y-1/2",
          arrow: "material-symbols-outlined text-secondary/40 text-[10px] bg-background px-1 relative z-10"
        }
      : {
          train: "font-semibold text-primary group-hover:text-primary-fixed transition-colors",
          line: "h-[1px] w-full bg-primary/20 absolute top-1/2 -translate-y-1/2",
          arrow: "material-symbols-outlined text-primary/40 text-[10px] bg-background px-1 relative z-10"
        };
    const waitlist = ticket.waitlist_available
      ? `<span class="text-tertiary font-medium">可候补</span>`
      : `<span class="text-on-surface-variant/50">不可候补</span>`;
    const createUrl = `/tasks/new?from=${encodeURIComponent(ticket.from_code)}&to=${encodeURIComponent(ticket.to_code)}&date=${encodeURIComponent(ticket.date)}&train=${encodeURIComponent(ticket.train_no)}`;
    return `<tr class="hover:bg-primary/[0.03] transition-colors group">
      <td class="p-4"><div class="${theme.train}">${escapeHtml(ticket.train_no)}</div></td>
      <td class="p-4"><div class="flex items-center gap-3">
        <div class="text-right"><div class="font-medium text-on-surface">${escapeHtml(ticket.depart_time)}</div><div class="text-xs text-on-surface-variant">${escapeHtml(ticket.from_name)}</div></div>
        <div class="w-8 flex flex-col items-center justify-center relative">
          <div class="${theme.line}"></div>
          <span class="${theme.arrow}">arrow_forward_ios</span>
        </div>
        <div><div class="font-medium text-on-surface">${escapeHtml(ticket.arrive_time)}</div><div class="text-xs text-on-surface-variant">${escapeHtml(ticket.to_name)}</div></div>
      </div></td>
      <td class="p-4"><div class="text-on-surface font-medium">${escapeHtml(ticket.duration)}</div></td>
      <td class="p-4 text-center">${renderAvailability(ticket.business)}</td>
      <td class="p-4 text-center">${renderAvailability(ticket.first_class)}</td>
      <td class="p-4 text-center">${renderAvailability(ticket.second_class)}</td>
      <td class="p-4 text-center">${renderAvailability(ticket.hard_sleeper)}</td>
      <td class="p-4 text-center">${renderAvailability(ticket.hard_seat)}</td>
      <td class="p-4 text-center">${renderAvailability(ticket.no_seat)}</td>
      <td class="p-4 text-center">${waitlist}</td>
      <td class="p-4 text-right">
        <button data-create-task-from-ticket="${createUrl}" class="px-3 py-1.5 text-xs font-medium bg-primary/10 text-primary border border-primary/20 hover:bg-primary/20 hover:border-primary/40 rounded transition-colors inline-flex items-center gap-1">
          <span class="material-symbols-outlined text-[14px]">add_task</span>创建抢票任务
        </button>
      </td>
    </tr>`;
  }

  async function runTicketQuery() {
    const inputs = Array.from(document.querySelectorAll("main input"));
    const from = parseStation(inputs[0]?.value, "北京", "BJP");
    const to = parseStation(inputs[1]?.value, "上海", "SHH");
    const date = inputs.find((input) => input.type === "date")?.value || "2026-07-15";
    const params = new URLSearchParams({ from: from.code, to: to.code, date });
    const tickets = await fetchJson(`/api/tickets/query?${params.toString()}`);
    const tbody = document.querySelector("tbody");
    if (!tbody) return;
    tbody.innerHTML = tickets.length
      ? tickets.map(renderTicketRow).join("")
      : `<tr><td colspan="11" class="p-10 text-center text-on-surface-variant">没有查询到符合条件的车次。</td></tr>`;
    document.querySelectorAll("[data-create-task-from-ticket]").forEach((button) => {
      button.addEventListener("click", () => { window.location.href = button.dataset.createTaskFromTicket; });
    });
    const countText = Array.from(document.querySelectorAll("span")).find((node) => normalize(node.textContent).startsWith("共找到"));
    if (countText) countText.textContent = `共找到 ${tickets.length} 趟列车`;
  }

  function hydrateTicketQueryPage() {
    if (window.location.pathname !== "/tickets") return;
    Array.from(document.querySelectorAll("main button"))
      .find((button) => normalize(button.textContent).includes("swap_horiz"))
      ?.addEventListener("click", () => swapInputValues(Array.from(document.querySelectorAll("main input[type='text']")).slice(0, 2)));
    const searchButton = Array.from(document.querySelectorAll("button")).find((button) => normalize(button.textContent) === "查询");
    searchButton?.addEventListener("click", (event) => {
      event.preventDefault();
      runTicketQuery().catch((error) => alert(`查询失败: ${error.message}`));
    });
  }

  function setLoginStatus(response) {
    const label = {
      logged_out: "当前状态：未登录",
      verification_required: "当前状态：需要人工验证",
      logged_in: "当前状态：已登录"
    }[response.state] || `当前状态：${response.state}`;
    const statusNode = Array.from(document.querySelectorAll("span"))
      .find((node) => normalize(node.textContent).startsWith("当前状态："));
    if (statusNode) statusNode.textContent = label;
  }

  function renderBlockedTask(task) {
    return `<li class="flex items-center justify-between text-xs p-2 bg-surface-container-highest/40 rounded border border-outline-variant/10">
      <button data-task-detail="${escapeHtml(task.id)}" class="flex items-center gap-2 text-on-surface-variant hover:text-primary transition-colors text-left">
        <span class="material-symbols-outlined text-[16px]">train</span>${escapeHtml(task.train_include[0] || "所有列车")} (${escapeHtml(task.from_name)} - ${escapeHtml(task.to_name)})
      </button>
      <span class="text-error/80">${escapeHtml(statusMeta[task.status]?.[1] || task.status)}</span>
    </li>`;
  }

  async function hydrateBlockedTasks() {
    const list = document.querySelector("ul.space-y-2");
    if (!list) return;
    const summaries = await fetchJson("/api/tasks");
    const details = await Promise.all(summaries.map((task) => loadTaskDetails(task.id)));
    const blocked = details.filter((task) => ["waiting_login", "verification_required"].includes(task.status));
    list.innerHTML = blocked.length
      ? blocked.map(renderBlockedTask).join("")
      : `<li class="text-xs p-2 bg-surface-container-highest/40 rounded border border-outline-variant/10 text-on-surface-variant">暂无受阻任务</li>`;
    list.querySelectorAll("[data-task-detail]").forEach((button) => {
      button.addEventListener("click", () => { window.location.href = `/tasks/${button.dataset.taskDetail}`; });
    });
  }

  async function hydrateLoginPage() {
    if (window.location.pathname !== "/login") return;
    setLoginStatus(await fetchJson("/api/session/status"));
    await hydrateBlockedTasks();
    const buttons = Array.from(document.querySelectorAll("button"));
    const loginButton = buttons.find((button) => normalize(button.textContent).includes("重新登录"));
    const verifyButton = buttons.find((button) => normalize(button.textContent).includes("处理验证"));
    loginButton?.addEventListener("click", async (event) => {
      event.preventDefault();
      const username = document.getElementById("account")?.value || "";
      const password = document.getElementById("password")?.value || "";
      const response = await fetchJson("/api/session/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ username, password })
      });
      setLoginStatus(response);
    });
    verifyButton?.addEventListener("click", async (event) => {
      event.preventDefault();
      const response = await fetchJson("/api/session/verification/open", { method: "POST" });
      window.open(response.url, "_blank", "noopener");
      if (confirm("请在 12306 官方页面手动完成验证。完成后点击确定更新本地登录状态。")) {
        setLoginStatus(await fetchJson("/api/session/verification/complete", { method: "POST" }));
      }
    });
  }

    async function hydrateSettingsPage() {
    if (window.location.pathname !== "/settings") return;
    const settings = await fetchJson("/api/settings");
    const values = {
      "db-path": settings.database_path,
      "listen-addr": settings.host,
      "listen-port": settings.port,
      "query-interval": settings.query_interval_ms,
      "log-level": settings.log_level
    };
    Object.entries(values).forEach(([id, value]) => {
      const input = document.getElementById(id);
      if (input) input.value = value;
    });
    const interval = document.getElementById("query-interval");
    if (interval) interval.min = settings.min_query_interval_ms;
    const saveButton = Array.from(document.querySelectorAll("button")).find((button) => normalize(button.textContent).includes("保存更改"));
    saveButton?.addEventListener("click", async (event) => {
      event.preventDefault();
      const payload = {
        host: document.getElementById("listen-addr")?.value || settings.host,
        port: Number(document.getElementById("listen-port")?.value || settings.port),
        database_path: document.getElementById("db-path")?.value || settings.database_path,
        query_interval_ms: Number(document.getElementById("query-interval")?.value || settings.query_interval_ms),
        log_level: document.getElementById("log-level")?.value || settings.log_level
      };
      try {
        await fetchJson("/api/settings", {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(payload)
        });
        alert("设置已保存。监听地址、端口、数据库路径和日志级别需重启服务后生效。");
      } catch (error) {
        alert(`保存设置失败: ${error.message}`);
      }
    });
  }

  function setDashboardCard(label, count, subtext) {
    const card = Array.from(document.querySelectorAll("a")).find((node) => normalize(node.textContent).includes(label));
    if (!card) return;
    const countNode = card.querySelector("h2");
    if (countNode) countNode.textContent = String(count);
    const subNode = card.querySelector("p");
    if (subNode && subtext) subNode.textContent = subtext;
  }

  async function hydrateDashboardPage() {
    if (window.location.pathname !== "/") return;
    const summaries = await fetchJson("/api/tasks");
    const details = await Promise.all(summaries.map((task) => loadTaskDetails(task.id)));
    const pendingPayment = details.filter((task) => ["pending_payment", "candidate_pending_payment"].includes(task.status)).length;
    const blocked = details.filter((task) => ["waiting_login", "verification_required"].includes(task.status)).length;
    const waitlist = details.filter((task) => ["candidate_submitting", "candidate_submitted", "candidate_pending_payment"].includes(task.status)).length;
    setDashboardCard("普通订单待支付", pendingPayment, pendingPayment ? "请前往官方渠道手动支付" : "暂无待支付订单");
    setDashboardCard("需人工验证", blocked, blocked ? "请打开官方验证页面手动处理" : "暂无验证阻塞任务");
    setDashboardCard("候补已提交等待兑现", waitlist, waitlist ? "候补状态请持续关注" : "暂无候补监控");
    const taskTableBody = Array.from(document.querySelectorAll("tbody")).at(-1);
    if (taskTableBody) {
      taskTableBody.innerHTML = details.length
        ? details.slice(0, 6).map(renderDashboardTaskRow).join("")
        : `<tr><td colspan="3" class="py-10 px-4 text-center text-on-surface-variant">暂无监控任务。</td></tr>`;
      taskTableBody.querySelectorAll("[data-task-detail]").forEach((button) => {
        button.addEventListener("click", () => { window.location.href = `/tasks/${button.dataset.taskDetail}`; });
      });
    }
  }

  function replaceLeafText(from, to) {
    document.querySelectorAll("span, p, h3, li").forEach((node) => {
      if (normalize(node.textContent) === normalize(from)) node.textContent = to;
    });
  }

  function updateDetailStatusCopy(task) {
    const label = statusMeta[task.status]?.[1] || task.status;
    replaceLeafText("候补兑现成功待支付", label);
    replaceLeafText("等待支付", label);
    document.querySelectorAll("li").forEach((item) => {
      const text = normalize(item.textContent);
      if (text.startsWith("状态:")) item.innerHTML = `<span class="w-1 h-1 rounded-full bg-error"></span> 状态: ${escapeHtml(label)}`;
      if (text.startsWith("目标车次:")) item.innerHTML = `<span class="w-1 h-1 rounded-full bg-tertiary"></span> 目标车次: ${escapeHtml(task.train_include[0] || "所有列车")}`;
    });
    const actionTitle = Array.from(document.querySelectorAll("h3")).find((node) => normalize(node.textContent).includes("ACTIONREQUIRED"));
    if (!actionTitle) return;
    const badge = actionTitle.querySelector("span")?.outerHTML || "";
    if (task.status === "pending_payment") {
      actionTitle.innerHTML = `待支付 - 普通订单已提交 ${badge}`;
    } else if (task.status === "candidate_pending_payment") {
      actionTitle.innerHTML = `待支付 - 候补兑现成功 ${badge}`;
    } else {
      actionTitle.innerHTML = `当前状态 - ${escapeHtml(label)} ${badge}`;
    }
    const description = actionTitle.parentElement?.querySelector("p");
    if (!description) return;
    if (task.status === "pending_payment" || task.status === "candidate_pending_payment") {
      description.innerHTML = `订单已提交，请前往 12306 官方客户端完成支付（系统不自动支付）。<br><span class="text-on-surface-variant">支付需要由用户手动完成，系统只保留提醒和状态追踪。</span>`;
    } else {
      description.innerHTML = `当前任务状态为 <strong class="text-on-surface">${escapeHtml(label)}</strong>。系统会按任务配置记录状态与日志，不会执行自动支付。`;
    }
  }

  function wireDetailTaskActions(task) {
    document.getElementById("detail-task-actions")?.remove();
    const statusPill = Array.from(document.querySelectorAll("span")).find((node) => normalize(node.textContent) === normalize(statusMeta[task.status]?.[1] || task.status));
    const statusBox = statusPill?.closest("div.flex.items-center.gap-3");
    if (!statusBox?.parentElement) return;
    const actions = document.createElement("div");
    actions.id = "detail-task-actions";
    actions.className = "flex items-center gap-1";
    actions.innerHTML = taskActionButtons(task);
    statusBox.parentElement.appendChild(actions);
    actions.querySelectorAll("[data-task-action]").forEach((button) => {
      button.addEventListener("click", async () => {
        await fetchJson(`/api/tasks/${button.dataset.taskId}/${button.dataset.taskAction}`, { method: "POST" });
        await hydrateTaskDetailPage();
      });
    });
  }

  async function hydrateTaskDetailPage() {
    const match = window.location.pathname.match(/^\/tasks\/([^/]+)$/);
    if (!match || match[1] === "demo") return;
    let task;
    try {
      task = await loadTaskDetails(match[1]);
    } catch (_) {
      return;
    }
    const title = document.querySelector("main h2");
    if (title) title.textContent = `任务运行控制台: ${task.from_name} 到 ${task.to_name}`;
    const subtitle = title?.parentElement?.querySelector("p");
    if (subtitle) subtitle.textContent = `创建于 ${task.created_at} · 目标站点: ${task.from_name} ➔ ${task.to_name}`;
    const statusPill = Array.from(document.querySelectorAll("span")).find((node) => normalize(node.textContent) === "候补兑现成功待支付");
    if (statusPill) statusPill.textContent = statusMeta[task.status]?.[1] || task.status;
    updateDetailStatusCopy(task);
    wireDetailTaskActions(task);
    setConfigValue("乘车人", `<span class="material-symbols-outlined text-[16px] text-primary/70">person</span>${escapeHtml(task.passenger_ids.length ? `${task.passenger_ids.length} 位乘车人` : "-")}`);
    setConfigValue("车次", `<div class="flex flex-wrap gap-1 mt-1">${(task.train_include.length ? task.train_include : ["所有列车"]).map(renderToken).join("")}</div>`);
    setConfigValue("坐席", escapeHtml(task.seat_types.length ? task.seat_types.map(seatLabel).join(", ") : (task.accept_no_seat ? "无座" : "-")));
    setConfigValue("日期", escapeHtml(task.dates.join(", ")));
    setConfigValue("新增车次", escapeHtml(newTrainPolicyLabel(task)));
    setConfigValue("轮询间隔", `${escapeHtml(task.query_interval_ms)}ms`);
    const newTrains = await fetchJson(`/api/tasks/${encodeURIComponent(task.id)}/new-trains`);
    const newTrainList = document.getElementById("new-train-observations");
    if (newTrainList) {
      newTrainList.innerHTML = newTrains.length
        ? newTrains.map((train) => `<div class="flex items-center justify-between gap-2">
            <span class="font-mono">${escapeHtml(train.train_no)}</span>
            <span class="text-xs text-on-surface-variant">${escapeHtml(train.travel_date)}</span>
          </div>`).join("")
        : "尚未发现";
    }
    const logs = await fetchJson(`/api/tasks/${encodeURIComponent(task.id)}/logs`);
    const terminal = document.getElementById("terminal-body");
    if (terminal && logs.length) {
      terminal.innerHTML = logs.map((log) => `<div class="flex gap-3 text-on-surface-variant">
        <span class="shrink-0 text-outline">${escapeHtml(log.created_at)}</span>
        <span class="text-secondary shrink-0">[${escapeHtml(log.level.toUpperCase())}]</span>
        <span class="break-all">${escapeHtml(log.message)}</span>
      </div>`).join("");
    }
  }

  hydrateTaskListPage().catch(console.error);
  hydrateTaskDetailPage().catch(console.error);
  hydrateCreateTaskPage().catch(console.error);
  hydrateTicketQueryPage();
  hydrateLoginPage().catch(console.error);
  hydrateSettingsPage().catch(console.error);
  hydrateDashboardPage().catch(console.error);
})();
</script>
</body>"#;

async fn settings(State(state): State<AppState>) -> Result<Json<SettingsResponse>, ApiError> {
    Ok(Json(settings_response(&state)?))
}

async fn update_settings(
    State(state): State<AppState>,
    Json(request): Json<SettingsUpdateRequest>,
) -> Result<Json<SettingsResponse>, ApiError> {
    let host = request.host.trim();
    let database_path = request.database_path.trim();
    let log_level = request.log_level.trim().to_lowercase();

    if host.is_empty() {
        return Err(ApiError::BadRequest("host is required".to_string()));
    }
    if request.port == 0 {
        return Err(ApiError::BadRequest(
            "port must be between 1 and 65535".to_string(),
        ));
    }
    if database_path.is_empty() {
        return Err(ApiError::BadRequest(
            "database_path is required".to_string(),
        ));
    }
    if request.query_interval_ms < MIN_QUERY_INTERVAL_MS {
        return Err(ApiError::BadRequest(format!(
            "query_interval_ms must be at least {MIN_QUERY_INTERVAL_MS}ms"
        )));
    }
    if !matches!(
        log_level.as_str(),
        "trace" | "debug" | "info" | "warn" | "error"
    ) {
        return Err(ApiError::BadRequest(
            "log_level must be trace, debug, info, warn, or error".to_string(),
        ));
    }

    state.database.set_setting("host", host)?;
    state
        .database
        .set_setting("port", &request.port.to_string())?;
    state.database.set_setting("database_path", database_path)?;
    state
        .database
        .set_setting("query_interval_ms", &request.query_interval_ms.to_string())?;
    state.database.set_setting("log_level", &log_level)?;

    Ok(Json(settings_response(&state)?))
}

async fn session_status(
    State(state): State<AppState>,
) -> Result<Json<SessionStatusResponse>, ApiError> {
    Ok(Json(session_response(state.database.session_state()?)))
}

async fn session_login(
    State(state): State<AppState>,
    Json(request): Json<SessionLoginRequest>,
) -> Result<Json<SessionStatusResponse>, ApiError> {
    if request.username.trim().is_empty() || request.password.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "username and password are required".to_string(),
        ));
    }
    let result = login_12306(LoginRequest {
        username: request.username,
        password: request.password,
    })
    .await
    .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let session_state = match result.result {
        LoginResult::LoggedIn => "logged_in",
        LoginResult::VerificationRequired { .. } => "verification_required",
        LoginResult::Failed { .. } => "failed",
    };
    state
        .database
        .set_session(session_state, result.cookies.as_deref())?;
    Ok(Json(session_response(session_state.to_string())))
}

async fn session_logout(
    State(state): State<AppState>,
) -> Result<Json<SessionStatusResponse>, ApiError> {
    state.database.set_session_state("logged_out")?;
    Ok(Json(session_response("logged_out".to_string())))
}

async fn session_verification_open() -> Json<VerificationOpenResponse> {
    Json(VerificationOpenResponse {
        url: "https://kyfw.12306.cn/otn/resources/login.html".to_string(),
    })
}

async fn session_verification_complete(
    State(state): State<AppState>,
) -> Result<Json<SessionStatusResponse>, ApiError> {
    state.database.set_session_state("logged_in")?;
    Ok(Json(session_response("logged_in".to_string())))
}

async fn query_tickets(
    Query(params): Query<TicketQueryParams>,
) -> Result<Json<Vec<TicketQueryRow>>, ApiError> {
    query_12306_tickets(&params.from, &params.to, params.date)
        .await
        .map(Json)
        .map_err(ApiError::BadRequest)
}

async fn list_passengers(State(state): State<AppState>) -> Result<Json<Vec<Passenger>>, ApiError> {
    Ok(Json(state.database.list_passengers()?))
}

async fn save_passenger(
    State(state): State<AppState>,
    Json(request): Json<SavePassengerRequest>,
) -> Result<(StatusCode, Json<Passenger>), ApiError> {
    let passenger = request.into_passenger()?;
    state.database.save_passenger(&passenger)?;
    Ok((StatusCode::CREATED, Json(passenger)))
}

pub async fn query_12306_tickets(
    from: &str,
    to: &str,
    date: NaiveDate,
) -> Result<Vec<TicketQueryRow>, String> {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .user_agent("Mozilla/5.0")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let stations = if let Some(stations) = STATIONS.get() {
        stations.clone()
    } else {
        let stations = fetch_stations(&client).await?;
        let _ = STATIONS.set(stations.clone());
        stations
    };
    let from = find_station(&stations, from)?;
    let to = find_station(&stations, to)?;

    client
        .get("https://kyfw.12306.cn/otn/leftTicket/init")
        .send()
        .await
        .map_err(|error| error.to_string())?;

    let response = client
        .get("https://kyfw.12306.cn/otn/leftTicket/queryG")
        .header("referer", "https://kyfw.12306.cn/otn/leftTicket/init")
        .query(&[
            ("leftTicketDTO.train_date", date.to_string()),
            ("leftTicketDTO.from_station", from.code.clone()),
            ("leftTicketDTO.to_station", to.code.clone()),
            ("purpose_codes", "ADULT".to_string()),
        ])
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("12306 query failed with HTTP {status}"));
    }
    let payload: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| "12306 returned a non-JSON response".to_string())?;
    let results = payload["data"]["result"]
        .as_array()
        .ok_or_else(|| payload["messages"].to_string())?;
    let names = payload["data"]["map"].as_object();
    Ok(results
        .iter()
        .filter_map(|value| value.as_str())
        .filter_map(|line| parse_left_ticket_row(line, names, date))
        .collect())
}

async fn list_tasks(State(state): State<AppState>) -> Result<Json<Vec<TaskSummary>>, ApiError> {
    Ok(Json(state.database.list_task_summaries()?))
}

async fn create_task(
    State(state): State<AppState>,
    Json(request): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<TaskDetails>), ApiError> {
    let task = request.into_task().map_err(ApiError::BadRequest)?;
    let task_id = task.id.0.to_string();
    state.database.save_task(&task)?;
    state.database.append_task_log(
        &task_id,
        "info",
        "task_created",
        "task created through Web API",
        None,
    )?;
    Ok((
        StatusCode::CREATED,
        Json(state.database.get_task_details(&task_id)?),
    ))
}

async fn get_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskDetails>, ApiError> {
    Ok(Json(state.database.get_task_details(&task_id)?))
}

async fn start_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskDetails>, ApiError> {
    update_task_status(state, task_id, TaskStatus::Running)
}

async fn pause_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskDetails>, ApiError> {
    update_task_status(state, task_id, TaskStatus::Paused)
}

async fn resume_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskDetails>, ApiError> {
    update_task_status(state, task_id, TaskStatus::Running)
}

async fn cancel_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskDetails>, ApiError> {
    update_task_status(state, task_id, TaskStatus::Cancelled)
}

async fn list_task_logs(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<TaskLog>>, ApiError> {
    Ok(Json(state.database.list_task_logs(&task_id)?))
}

async fn list_new_trains(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<NewTrainRecord>>, ApiError> {
    Ok(Json(state.database.list_new_trains(&task_id)?))
}

fn update_task_status(
    state: AppState,
    task_id: String,
    status: TaskStatus,
) -> Result<Json<TaskDetails>, ApiError> {
    Ok(Json(state.database.update_task_status(&task_id, status)?))
}

#[derive(Debug, Deserialize)]
struct CreateTaskRequest {
    from_name: String,
    from_code: String,
    to_name: String,
    to_code: String,
    dates: Vec<NaiveDate>,
    passenger_ids: Vec<Uuid>,
    seat_preferences: Vec<SeatType>,
    #[serde(default)]
    accept_no_seat: bool,
    #[serde(default)]
    train_include: Vec<String>,
    #[serde(default)]
    train_exclude: Vec<String>,
    #[serde(default)]
    enable_waitlist: bool,
    #[serde(default)]
    enable_strong_waitlist: bool,
    #[serde(default)]
    new_train_policy: NewTrainPolicy,
    #[serde(default)]
    new_trains_only: bool,
    #[serde(default)]
    query_interval_ms: Option<u64>,
    #[serde(default)]
    remark: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TicketQueryParams {
    from: String,
    to: String,
    date: NaiveDate,
}

#[derive(Debug, Deserialize)]
struct SessionLoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct SavePassengerRequest {
    #[serde(default)]
    id: Option<Uuid>,
    name: String,
    id_masked: String,
    passenger_type: PassengerType,
}

impl SavePassengerRequest {
    fn into_passenger(self) -> Result<Passenger, ApiError> {
        let name = self.name.trim();
        let id_masked = self.id_masked.trim();
        if name.is_empty() {
            return Err(ApiError::BadRequest("name is required".to_string()));
        }
        if id_masked.is_empty() {
            return Err(ApiError::BadRequest("id_masked is required".to_string()));
        }
        Ok(Passenger {
            id: PassengerId(self.id.unwrap_or_else(Uuid::new_v4)),
            name: name.to_string(),
            id_masked: id_masked.to_string(),
            passenger_type: self.passenger_type,
        })
    }
}

#[derive(Debug, Deserialize)]
struct SettingsUpdateRequest {
    host: String,
    port: u16,
    database_path: String,
    query_interval_ms: u64,
    log_level: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TicketQueryRow {
    pub secret_str: String,
    pub train_id: String,
    pub train_no: String,
    pub from_code: String,
    pub from_name: String,
    pub to_code: String,
    pub to_name: String,
    pub date: String,
    pub depart_time: String,
    pub arrive_time: String,
    pub duration: String,
    pub can_web_buy: bool,
    pub business: String,
    pub first_class: String,
    pub second_class: String,
    pub soft_sleeper: String,
    pub hard_sleeper: String,
    pub hard_seat: String,
    pub no_seat: String,
    pub waitlist_available: bool,
    pub waitlist_seat_codes: String,
}

impl TicketQueryRow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        train_no: &str,
        train_id: &str,
        secret_str: &str,
        from_code: &str,
        to_code: &str,
        date: NaiveDate,
        from_name: &str,
        to_name: &str,
        depart_time: &str,
        arrive_time: &str,
        duration: &str,
        can_web_buy: bool,
        seats: [&str; 7],
        waitlist_available: bool,
        waitlist_seat_codes: &str,
    ) -> Self {
        Self {
            secret_str: secret_str.to_string(),
            train_id: train_id.to_string(),
            train_no: train_no.to_string(),
            from_code: from_code.to_string(),
            from_name: from_name.to_string(),
            to_code: to_code.to_string(),
            to_name: to_name.to_string(),
            date: date.to_string(),
            depart_time: depart_time.to_string(),
            arrive_time: arrive_time.to_string(),
            duration: duration.to_string(),
            can_web_buy,
            business: seats[0].to_string(),
            first_class: seats[1].to_string(),
            second_class: seats[2].to_string(),
            soft_sleeper: seats[3].to_string(),
            hard_sleeper: seats[4].to_string(),
            hard_seat: seats[5].to_string(),
            no_seat: seats[6].to_string(),
            waitlist_available,
            waitlist_seat_codes: waitlist_seat_codes.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct RailwayStation {
    name: String,
    code: String,
    pinyin: String,
    short: String,
}

static STATIONS: OnceLock<Vec<RailwayStation>> = OnceLock::new();

async fn fetch_stations(client: &reqwest::Client) -> Result<Vec<RailwayStation>, String> {
    const URL: &str = "https://kyfw.12306.cn/otn/resources/js/framework/station_name.js";
    let mut last_error = String::new();
    for attempt in 1..=3 {
        let result = async {
            let response = client.get(URL).send().await.map_err(request_error)?;
            if !response.status().is_success() {
                return Err(format!("HTTP {}", response.status()));
            }
            let text = response.text().await.map_err(request_error)?;
            let stations = parse_stations(&text);
            if stations.is_empty() {
                return Err("12306 returned an empty station list".to_string());
            }
            Ok(stations)
        }
        .await;
        match result {
            Ok(stations) => return Ok(stations),
            Err(error) => last_error = error,
        }
        if attempt < 3 {
            tokio::time::sleep(Duration::from_millis(300 * attempt)).await;
        }
    }
    Err(format!(
        "failed to download 12306 station list after 3 attempts: {last_error}"
    ))
}

fn request_error(error: reqwest::Error) -> String {
    format!(
        "{error} (connect={}, timeout={})",
        error.is_connect(),
        error.is_timeout()
    )
}

fn parse_stations(text: &str) -> Vec<RailwayStation> {
    text.split('@')
        .filter_map(|record| {
            let fields: Vec<_> = record.split('|').collect();
            Some(RailwayStation {
                name: fields.get(1)?.to_string(),
                code: fields.get(2)?.to_string(),
                pinyin: fields.get(3)?.to_string(),
                short: fields.get(4)?.to_string(),
            })
        })
        .collect()
}

fn find_station(stations: &[RailwayStation], input: &str) -> Result<RailwayStation, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("station is required".to_string());
    }
    let upper = input.to_uppercase();
    let lower = input.to_lowercase();
    stations
        .iter()
        .find(|station| {
            station.code == upper
                || station.name == input
                || station.pinyin == lower
                || station.short == lower
        })
        .cloned()
        .ok_or_else(|| format!("unknown station: {input}"))
}

fn parse_left_ticket_row(
    line: &str,
    names: Option<&serde_json::Map<String, serde_json::Value>>,
    date: NaiveDate,
) -> Option<TicketQueryRow> {
    let fields: Vec<_> = line.split('|').collect();
    let secret_str = fields.first().copied().unwrap_or_default();
    let train_id = fields.get(2).copied().unwrap_or_default();
    let train_no = fields.get(3)?;
    let from_code = fields.get(6)?;
    let to_code = fields.get(7)?;
    let station_name = |code: &str| {
        names
            .and_then(|map| map.get(code))
            .and_then(|value| value.as_str())
            .unwrap_or(code)
            .to_string()
    };
    Some(TicketQueryRow::new(
        train_no,
        train_id,
        secret_str,
        from_code,
        to_code,
        date,
        &station_name(from_code),
        &station_name(to_code),
        fields.get(8).copied().unwrap_or("--"),
        fields.get(9).copied().unwrap_or("--"),
        fields.get(10).copied().unwrap_or("--"),
        fields.get(11).is_some_and(|value| *value == "Y"),
        [
            clean_seat(fields.get(32).copied()),
            clean_seat(fields.get(31).copied()),
            clean_seat(fields.get(30).copied()),
            clean_seat(fields.get(23).copied()),
            clean_seat(fields.get(28).copied()),
            clean_seat(fields.get(29).copied()),
            clean_seat(fields.get(26).copied()),
        ],
        fields.get(37).is_some_and(|value| *value == "1"),
        fields.get(38).copied().unwrap_or_default(),
    ))
}

fn clean_seat(value: Option<&str>) -> &str {
    match value.unwrap_or("--").trim() {
        "" => "--",
        value => value,
    }
}

impl CreateTaskRequest {
    fn into_task(self) -> Result<rs12306_core::TicketTask, String> {
        NewTicketTask {
            from: Station {
                name: self.from_name,
                code: self.from_code,
            },
            to: Station {
                name: self.to_name,
                code: self.to_code,
            },
            dates: self.dates,
            passengers: self.passenger_ids.into_iter().map(PassengerId).collect(),
            seat_preferences: self.seat_preferences,
            accept_no_seat: self.accept_no_seat,
            train_filters: parse_train_filters(self.train_include, self.train_exclude),
            enable_waitlist: self.enable_waitlist,
            enable_strong_waitlist: self.enable_strong_waitlist,
            new_train_policy: self.new_train_policy,
            new_trains_only: self.new_trains_only,
            query_interval_ms: self.query_interval_ms,
            remark: self.remark,
        }
        .build()
        .map_err(|error| error.to_string())
    }
}

fn parse_train_filters(include: Vec<String>, exclude: Vec<String>) -> Vec<TrainFilter> {
    let includes = include.into_iter().map(|train_no| TrainFilter {
        kind: TrainFilterKind::Include,
        train_no,
    });
    let excludes = exclude.into_iter().map(|train_no| TrainFilter {
        kind: TrainFilterKind::Exclude,
        train_no,
    });
    includes.chain(excludes).collect()
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
}

#[derive(Debug, Serialize)]
struct SettingsResponse {
    host: String,
    port: u16,
    database_path: String,
    query_interval_ms: u64,
    min_query_interval_ms: u64,
    log_level: String,
}

fn settings_response(state: &AppState) -> Result<SettingsResponse, ApiError> {
    let host = state
        .database
        .get_setting("host")?
        .unwrap_or_else(|| state.config.host.clone());
    let port = state
        .database
        .get_setting("port")?
        .and_then(|value| value.parse().ok())
        .unwrap_or(state.config.port);
    let database_path = state
        .database
        .get_setting("database_path")?
        .unwrap_or_else(|| state.config.database_path.display().to_string());
    let query_interval_ms = state
        .database
        .get_setting("query_interval_ms")?
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_QUERY_INTERVAL_MS);
    let log_level = state
        .database
        .get_setting("log_level")?
        .unwrap_or_else(|| std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()));

    Ok(SettingsResponse {
        host,
        port,
        database_path,
        query_interval_ms,
        min_query_interval_ms: MIN_QUERY_INTERVAL_MS,
        log_level,
    })
}

#[derive(Debug, Serialize)]
struct SessionStatusResponse {
    state: String,
    verification_required: bool,
}

#[derive(Debug, Serialize)]
struct VerificationOpenResponse {
    url: String,
}

fn session_response(state: String) -> SessionStatusResponse {
    SessionStatusResponse {
        verification_required: state == "verification_required",
        state,
    }
}

#[derive(Debug)]
pub enum ApiError {
    Storage(StorageError),
    BadRequest(String),
}

impl From<StorageError> for ApiError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Storage(StorageError::TaskNotFound(task_id)) => {
                (StatusCode::NOT_FOUND, format!("task not found: {task_id}"))
            }
            Self::Storage(StorageError::InvalidStatusTransition { from, to }) => (
                StatusCode::CONFLICT,
                format!("invalid task status transition: {from} -> {to}"),
            ),
            Self::Storage(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
        };
        (status, Json(ErrorResponse { error: message })).into_response()
    }
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_documented_defaults() {
        let config = ServerConfig::default();

        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 12306);
        assert_eq!(DEFAULT_QUERY_INTERVAL_MS, 3_000);
        assert_eq!(MIN_QUERY_INTERVAL_MS, 1_000);
    }

    #[test]
    fn parses_station_list_records() {
        let stations = parse_stations(
            "var station_names ='@shh|上海|SHH|shanghai|sh|0@jxh|嘉兴|JXH|jiaxing|jx|1';",
        );

        assert_eq!(stations.len(), 2);
        assert_eq!(stations[0].name, "上海");
        assert_eq!(stations[0].code, "SHH");
        assert_eq!(stations[1].pinyin, "jiaxing");
    }
}
