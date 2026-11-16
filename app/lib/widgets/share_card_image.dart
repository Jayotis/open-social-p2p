import 'dart:math';
import 'dart:ui' as ui;

import 'package:flutter/painting.dart';
import 'package:flutter/services.dart';

import '../format.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';

/// The card's size in pixels: 4:5, as photo feeds show it whole.
const shareCardSize = Size(1080, 1350);

/// What the card says, line by line (ADR 0039 §1): the people found, then
/// what's worth showing of the rest.
List<String> shareCardLines(ShareCard card) {
  String n(int count, String one, String many) =>
      '$count ${count == 1 ? one : many}';
  final people = n(
    card.people.toInt(),
    'person discovered',
    'people discovered',
  );
  final direct = card.direct.toInt();
  return [
    direct > 0
        ? '$people · ${n(direct, 'direct connection', 'direct connections')}'
        : people,
    if (card.paths.toInt() > 0)
      n(
        card.paths.toInt(),
        'separate path into the wider network',
        'separate paths into the wider network',
      ),
    if (card.bytesServed.toInt() > 0)
      '${formatBytes(card.bytesServed.toInt())} served peer to peer',
  ];
}

/// The names on [card], as it draws them.
List<String> shareCardNames(ShareCard card) => [
  for (final p in card.points) ?p.name,
];

/// Draws [card] as a PNG: what the preview shows and the share sheet sends,
/// the same bytes. Only what [card] holds is drawn, so the picture can't
/// say more than the node's allowlist let through.
Future<Uint8List> renderShareCard(ShareCard card) async {
  final mark = await _mark();
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder);
  _paint(canvas, card, mark);
  final image = await recorder.endRecording().toImage(
    shareCardSize.width.toInt(),
    shareCardSize.height.toInt(),
  );
  final data = await image.toByteData(format: ui.ImageByteFormat.png);
  image.dispose();
  return data!.buffer.asUint8List();
}

Future<ui.Image?> _mark() async {
  try {
    final bytes = await rootBundle.load('assets/brand/mark.png');
    final codec = await ui.instantiateImageCodec(
      bytes.buffer.asUint8List(),
      targetWidth: 112,
    );
    return (await codec.getNextFrame()).image;
  } catch (_) {
    return null;
  }
}

const _colors = OspColors.night;

void _paint(Canvas canvas, ShareCard card, ui.Image? mark) {
  final size = shareCardSize;
  final all = Offset.zero & size;
  canvas.drawRect(
    all,
    Paint()
      ..shader = const LinearGradient(
        begin: Alignment.topCenter,
        end: Alignment.bottomCenter,
        colors: [Palette.night, Palette.nightHaze],
      ).createShader(all),
  );
  // Dust: decoration, the same on every card.
  final random = Random(39);
  final dust = Paint();
  for (var i = 0; i < 260; i++) {
    dust.color = _colors.distantStar.withValues(
      alpha: 0.2 + random.nextDouble() * 0.5,
    );
    canvas.drawCircle(
      Offset(
        random.nextDouble() * size.width,
        random.nextDouble() * size.height,
      ),
      0.6 + random.nextDouble() * 1.6,
      dust,
    );
  }

  // The title, with the app's icon.
  const left = 80.0;
  var y = 80.0;
  if (mark != null) {
    canvas.drawImageRect(
      mark,
      Offset.zero & Size(mark.width.toDouble(), mark.height.toDouble()),
      Rect.fromLTWH(left, y, 112, 112),
      Paint()..filterQuality = FilterQuality.medium,
    );
  }
  _text(
    canvas,
    'Open Social P2P',
    Offset(mark != null ? left + 140 : left, y + 22),
    size: 60,
    weight: FontWeight.w700,
    color: _colors.onSky,
    width: size.width - left * 2 - 140,
  );
  y += 160;
  for (final (i, line) in shareCardLines(card).indexed) {
    y += _text(
      canvas,
      line,
      Offset(left, y),
      size: i == 0 ? 40 : 36,
      weight: i == 0 ? FontWeight.w600 : FontWeight.w400,
      color: i == 0 ? _colors.onSky : _colors.onSkyQuiet,
      width: size.width - left * 2,
    );
    y += 12;
  }

  // The drawing, fitted into the rest above the footer.
  final area = Rect.fromLTRB(
    left,
    y + 40,
    size.width - left,
    size.height - 130,
  );
  _drawing(canvas, card, area);

  _text(
    canvas,
    'A social network run from your own home node',
    Offset(left, size.height - 96),
    size: 30,
    color: _colors.onSkyQuiet,
    width: size.width - left * 2,
  );
}

void _drawing(Canvas canvas, ShareCard card, Rect area) {
  final points = card.points;
  if (points.isEmpty) return;
  var (minX, maxX, minY, maxY) = (0.0, 0.0, 0.0, 0.0);
  for (final p in points) {
    minX = min(minX, p.x);
    maxX = max(maxX, p.x);
    minY = min(minY, p.y);
    maxY = max(maxY, p.y);
  }
  // Room round the edge for a name under a point.
  final inner = area.deflate(60);
  final span = max(maxX - minX, maxY - minY);
  final scale = span == 0
      ? 1.0
      : min(inner.width / (maxX - minX + 1), inner.height / (maxY - minY + 1));
  final mid = Offset((minX + maxX) / 2, (minY + maxY) / 2);
  Offset at(CardPoint p) =>
      inner.center + (Offset(p.x, p.y) - mid) * min(scale, 3.0);

  final line = Paint()
    ..color = _colors.reshared.withValues(alpha: 0.45)
    ..strokeWidth = 2.5;
  for (final l in card.lines) {
    if (l.from >= points.length || l.to >= points.length) continue;
    canvas.drawLine(at(points[l.from]), at(points[l.to]), line);
  }
  for (final p in points) {
    final c = at(p);
    final (radius, color) = switch (p) {
      CardPoint(you: true) => (16.0, _colors.yourStar),
      CardPoint(people: > 1) => (10.0 + 4 * log(p.people) / ln2, _colors.group),
      _ => (8.0, _colors.near),
    };
    canvas.drawCircle(
      c,
      radius * 3,
      Paint()
        ..shader = RadialGradient(
          colors: [
            color.withValues(alpha: p.you ? 0.45 : 0.25),
            color.withValues(alpha: 0),
          ],
        ).createShader(Rect.fromCircle(center: c, radius: radius * 3)),
    );
    canvas.drawCircle(c, radius, Paint()..color = color);
    if (p.name case final name?) {
      final w = 320.0;
      _text(
        canvas,
        name,
        c + Offset(-w / 2, radius + 8),
        size: 30,
        weight: FontWeight.w500,
        color: _colors.onSky,
        width: w,
        align: TextAlign.center,
      );
    }
  }
}

/// Draws one line of text and gives its height.
double _text(
  Canvas canvas,
  String text,
  Offset at, {
  required double size,
  required Color color,
  required double width,
  FontWeight weight = FontWeight.w400,
  TextAlign align = TextAlign.left,
}) {
  final painter = TextPainter(
    text: TextSpan(
      text: text,
      style: TextStyle(
        fontFamily: Fonts.family,
        fontSize: size,
        fontWeight: weight,
        color: color,
        height: 1.25,
      ),
    ),
    textAlign: align,
    textDirection: TextDirection.ltr,
    maxLines: 2,
    ellipsis: '…',
  )..layout(minWidth: width, maxWidth: width);
  painter.paint(canvas, at);
  final h = painter.height;
  painter.dispose();
  return h;
}
