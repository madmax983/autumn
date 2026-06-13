use autumn_web::model;

// ── State machine on a single field ──────────────────────────────────────────

diesel::table! {
    orders (id) {
        id -> BigInt,
        amount -> BigInt,
        status -> Text,
    }
}

#[model(table = "orders")]
pub struct Order {
    #[id]
    pub id: i64,
    pub amount: i64,
    #[state_machine(transitions(
        pending -> processing,
        processing -> shipped: "can_ship",
        processing -> cancelled,
        shipped -> delivered,
    ))]
    pub status: String,
}

impl Order {
    fn can_ship(&self) -> bool {
        self.amount > 0
    }
}

// ── Multiple state machine fields on the same model ──────────────────────────

diesel::table! {
    tickets (id) {
        id -> BigInt,
        status -> Text,
        priority -> Text,
    }
}

#[model(table = "tickets")]
pub struct Ticket {
    #[id]
    pub id: i64,
    #[state_machine(transitions(
        open -> in_progress: "can_start",
        in_progress -> closed,
    ))]
    pub status: String,
    #[state_machine(transitions(
        low -> medium,
        medium -> high,
    ))]
    pub priority: String,
}

impl Ticket {
    fn can_start(&self) -> bool {
        true
    }
}

// ── Unguarded single-transition machine ──────────────────────────────────────

diesel::table! {
    workflows (id) {
        id -> BigInt,
        phase -> Text,
    }
}

#[model(table = "workflows")]
pub struct Workflow {
    #[id]
    pub id: i64,
    #[state_machine(transitions(draft -> active, active -> archived))]
    pub phase: String,
}

fn _assert_can_transition_compiles(order: &Order) {
    let _ = order.can_transition_status_to("processing");
    let _ = order.transition_status_to("processing");
}

fn _assert_constant_accessible() {
    let _: &[(&str, &str, Option<&str>)] = Order::__AUTUMN_SM_STATUS_TRANSITIONS;
    let _: &[(&str, &str, Option<&str>)] = Ticket::__AUTUMN_SM_STATUS_TRANSITIONS;
    let _: &[(&str, &str, Option<&str>)] = Ticket::__AUTUMN_SM_PRIORITY_TRANSITIONS;
    let _: &[(&str, &str, Option<&str>)] = Workflow::__AUTUMN_SM_PHASE_TRANSITIONS;
}

fn main() {}
