"""Select one exact visible result from labels collected inside the private Shell."""


def exact_visible_result(labels, target, width, height):
    matches = {}
    for label in labels:
        if label["name"] != target or not label["showing"]:
            continue
        x, y, w, h = label["bounds"]
        if w > 0 and h > 0 and 0 <= x and 0 <= y and x + w <= width and y + h <= height:
            # AT-SPI may expose duplicate labels for the same Shell result.
            # Only identical screen bounds alias one selectable coordinate.
            matches.setdefault((x, y, w, h), label)
    assert len(matches) <= 1, "ambiguous exact visible GNOME result labels"
    return next(iter(matches.values()), None)
