use super::*;

/// Waiting messages can be put in a different order before they go.
pub(super) fn queue_order() -> Vec<(f32, Act)> {
    let mut v = new_session("a long answer please");
    v.extend([
        (0.1, Until("it streams", |d, _| streaming(d), 10.)),
        (0.1, Type(Field::Composer, "first later")),
        (0.1, Key("enter")),
        (0.2, Type(Field::Composer, "second later")),
        (0.1, Key("enter")),
        (
            0.1,
            Until(
                "two wait, and only the second can move up",
                |d, _| {
                    let q = queued_ids(d);
                    q.len() == 2
                        && drawn(d, &format!("move-up-{}", q[1].0)).is_some()
                        && drawn(d, &format!("move-up-{}", q[0].0)).is_none()
                },
                5.,
            ),
        ),
        (
            0.,
            Call(|d, _, cx| {
                let q = queued_ids(d);
                let sid = d.selected.clone().unwrap_or_default();
                if q.len() == 2 {
                    d.requeue(&sid, &q[1].1, Some(q[0].1.clone()), cx);
                }
            }),
        ),
        (
            0.1,
            Until(
                "Move up puts it first",
                |d, _| {
                    let q = queued_ids(d);
                    q.len() == 2 && q[0].2 == "second later"
                },
                3.,
            ),
        ),
        (0., Shot("queue")),
        (
            0.1,
            Until(
                "and they go in that order",
                |d, _| {
                    let o = sent_order(d);
                    status_is(d, Status::Done)
                        && queued_ids(d).is_empty()
                        && o.ends_with(&["second later".to_string(), "first later".to_string()])
                },
                40.,
            ),
        ),
    ]);
    v
}
