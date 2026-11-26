import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';
import '../../theme/appearance_controller.dart';
import '../../theme/skins.dart';
import '../../theme/theme_reveal.dart';
import '../../widgets/nav_rail_item.dart';

enum _SettingsSection {
  general,
  appearance,
  backup,
  webSecurity,
  serviceDiag,
  about,
}

/// 设置页（重构版，全面对标 Settings 参考图）：
/// - 移动端（< 840px）：单一纵向分组白卡列表（蓝字小节标题 + 圆角白卡），彻底取消顶部两排 6 药丸按钮。
/// - 桌面端（>= 840px）：左侧 5 项清爽分类 + 右侧最大宽度 720px 分组白卡。
/// - 所有开关即改即生效，彻底移除“保存备份与网络策略”按钮与重复的“立即校验”蓝按钮。
/// - 统一收口权限、主/备异盘目录、Web 访问密码保护（默认关闭可选开关）与应急重启服务。
class SettingsScreen extends StatefulWidget {
  const SettingsScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<SettingsScreen> createState() => _SettingsScreenState();
}

class _SettingsScreenState extends State<SettingsScreen>
    with WidgetsBindingObserver {
  TextEditingController? _ownedSearchController;

  TextEditingController get _searchController =>
      _ownedSearchController ??= TextEditingController();

  @override
  void dispose() {
    _ownedSearchController?.dispose();
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  Future<void> _applyAppearance(
    BuildContext triggerContext,
    VoidCallback change,
  ) async {
    await ThemeRevealHost.of(triggerContext).reveal(triggerContext, change);
  }

  void _showSkinMarket() {
    showNeuDialog<void>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setDialogState) {
          final appearance = AppearanceScope.of(context);
          final query = _searchController.text.trim().toLowerCase();
          final groups = query.isEmpty
              ? skinGroups
              : skinGroups
                  .map((group) => SkinGroup(
                        id: group.id,
                        label: group.label,
                        skins: group.skins.where((skin) {
                          return skin.name.toLowerCase().contains(query) ||
                              group.label.toLowerCase().contains(query);
                        }).toList(growable: false),
                      ))
                  .where((group) => group.skins.isNotEmpty)
                  .toList(growable: false);
          return NeuDialog(
            title: '皮肤市场',
            subtitle: '搜索名称或分类，点击色板即时预览并应用',
            maxWidth: 680,
            actions: <Widget>[
              NeuButton(
                label: '恢复柔冰蓝',
                onPressed: () {
                  _applyAppearance(
                      context, () => appearance.setSkin(defaultSkin));
                  Navigator.of(dialogContext).pop();
                },
              ),
              NeuButton(
                label: '完成',
                tone: NeuButtonTone.primary,
                onPressed: () => Navigator.of(dialogContext).pop(),
              ),
            ],
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                NeuInsetField(
                  controller: _searchController,
                  hint: '搜索皮肤，例如：赤陶、蓝色、舒适',
                  prefixIcon: Icons.search_rounded,
                  onSubmitted: (_) => setDialogState(() {}),
                ),
                const SizedBox(height: 12),
                Flexible(
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(maxHeight: 420),
                    child: SingleChildScrollView(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: <Widget>[
                          for (final group in groups) ...<Widget>[
                            Padding(
                              padding: const EdgeInsets.fromLTRB(2, 10, 2, 7),
                              child: Text(
                                group.label,
                                style: Theme.of(context)
                                    .textTheme
                                    .labelMedium
                                    ?.copyWith(
                                        color: NeuTheme.of(context).accent),
                              ),
                            ),
                            Wrap(
                              spacing: 8,
                              runSpacing: 8,
                              children: <Widget>[
                                for (final skin in group.skins)
                                  _SkinMarketCard(
                                    skin: skin,
                                    selected: appearance.skinId == skin.id,
                                    onTap: () => _applyAppearance(
                                      context,
                                      () => appearance.setSkin(skin),
                                    ),
                                  ),
                              ],
                            ),
                          ],
                          if (groups.isEmpty)
                            Padding(
                              padding: const EdgeInsets.all(24),
                              child: Text(
                                '没有匹配的皮肤，请换一个关键词。',
                                textAlign: TextAlign.center,
                                style: Theme.of(context).textTheme.bodyMedium,
                              ),
                            ),
                        ],
                      ),
                    ),
                  ),
                ),
              ],
            ),
          );
        },
      ),
    );
  }

  Widget _buildAppearanceGroup(BuildContext context) {
    final appearance =
        AppearanceScope.maybeOf(context) ?? AppearanceController.memory();
    final neu = NeuTheme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 16, 8),
          child: Text('日夜模式', style: Theme.of(context).textTheme.titleSmall),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: NeuSegmentedControl<ThemeModePreference>(
            semanticLabel: '日夜模式',
            value: appearance.themeMode,
            segments: const <NeuSegment<ThemeModePreference>>[
              NeuSegment(value: ThemeModePreference.system, label: '跟随系统'),
              NeuSegment(value: ThemeModePreference.light, label: '日间'),
              NeuSegment(value: ThemeModePreference.dark, label: '夜间'),
            ],
            onChanged: (mode) => _applyAppearance(
              context,
              () => appearance.setThemeMode(mode),
            ),
          ),
        ),
        const SizedBox(height: 14),
        _SettingsValueRow(
          title: '当前皮肤',
          subtitle: '${appearance.activeSkin.name} · 双日夜调板',
          value: appearance.activeSkin.family == SkinFamily.duo ? '双色' : '单色',
          icon: Icons.palette_outlined,
          onTap: _showSkinMarket,
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 14),
          child: Row(
            children: <Widget>[
              for (final color in appearance.activeSkin.swatches) ...<Widget>[
                Container(
                  width: 18,
                  height: 18,
                  decoration: BoxDecoration(
                    color: color,
                    shape: BoxShape.circle,
                    border: Border.all(color: neu.hairlineBorder),
                  ),
                ),
                const SizedBox(width: 6),
              ],
              const Spacer(),
              NeuButton(
                label: '更换皮肤',
                icon: Icons.storefront_outlined,
                onPressed: _showSkinMarket,
              ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
          child: Text(
            '滑动与交互动画速度',
            style: Theme.of(context).textTheme.titleSmall,
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 14),
          child: NeuSegmentedControl<MotionSpeed>(
            semanticLabel: '动画速度',
            value: appearance.motionSpeed,
            segments: <NeuSegment<MotionSpeed>>[
              for (final speed in MotionSpeed.values)
                NeuSegment(value: speed, label: speed.label),
            ],
            onChanged: appearance.setMotionSpeed,
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 14),
          child: Row(
            children: <Widget>[
              Icon(Icons.devices_rounded, size: 18, color: neu.textSecondary),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  'Windows 标题栏与 Android 系统栏会自动同步当前日夜模式；系统开启减弱动画时自动降级。',
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }

  _SettingsSection _desktopSection = _SettingsSection.general;

  RoleMode _roleMode = RoleMode.dual;
  bool _autoReconnect = true;
  bool _autostartServer = true;
  bool _bitRotAutoScrub = true;
  bool _scrubbing = false;
  bool _debugMode = false;
  bool _webPasswordEnabled = false;

  PowerNetworkPolicyModel _policy = const PowerNetworkPolicyModel(
    wifiOnly: true,
    allowedSsids: <String>[],
    requireCharging: false,
    maxKbps: 0,
  );
  BackupStorageConfigModel? _storageConfig;
  LastServerConfig? _lastServerConfig;
  SystemOverviewModel? _overview;
  bool _restarting = false;
  bool _runningDiag = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    unawaited(_loadAll());
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed && Platform.isAndroid) {
      unawaited(_refreshAndroidPermissions());
    }
  }

  Future<void> _refreshAndroidPermissions() async {
    if (!Platform.isAndroid) return;
    try {
      await widget.bridge.getAndroidPermissionStates();
      if (!mounted) return;
      setState(() {});
    } catch (_) {}
  }

  List<String> _loadFailures = const <String>[];

  Future<void> _loadAll() async {
    final failures = <String>[];

    Future<void> loadItem(String label, Future<void> Function() task) async {
      try {
        await task();
      } catch (_) {
        failures.add(label);
      }
    }

    await Future.wait<void>(<Future<void>>[
      loadItem('节点模式', () async {
        final role = await widget.bridge.getRoleMode();
        if (mounted) setState(() => _roleMode = role);
      }),
      loadItem('自动重连', () async {
        final autoRec = await widget.bridge.getAutoReconnect();
        if (mounted) setState(() => _autoReconnect = autoRec);
      }),
      loadItem('自动启动服务', () async {
        final autostart = await widget.bridge.getAutostartServer();
        if (mounted) setState(() => _autostartServer = autostart);
      }),
      loadItem('网络与电源策略', () async {
        final policy = await widget.bridge.getPowerNetworkPolicy();
        if (mounted) setState(() => _policy = policy);
      }),
      loadItem('自动完整性校验', () async {
        final scrub = await widget.bridge.getBitRotAutoScrub();
        if (mounted) setState(() => _bitRotAutoScrub = scrub);
      }),
      loadItem('调试模式', () async {
        final debug = await widget.bridge.getDebugMode();
        if (mounted) setState(() => _debugMode = debug);
      }),
      loadItem('存储目录配置', () async {
        final backupCfg = await widget.bridge.getBackupStorageConfig();
        if (mounted) setState(() => _storageConfig = backupCfg);
      }),
      loadItem('最近服务配置', () async {
        final lastCfg = await widget.bridge.getLastServerConfig();
        if (mounted) {
          setState(() {
            _lastServerConfig = lastCfg;
            _webPasswordEnabled = lastCfg?.webPassword != null &&
                lastCfg!.webPassword!.isNotEmpty;
          });
        }
      }),
      loadItem('系统概览', () async {
        final overview = await widget.bridge.getSystemOverview();
        if (mounted) setState(() => _overview = overview);
      }),
      if (Platform.isAndroid) _refreshAndroidPermissions(),
    ]);

    if (!mounted) return;
    setState(() => _loadFailures = failures);
    if (failures.isNotEmpty) {
      NeuToast.warning(
        context,
        '部分设置加载失败：${failures.join('、')}',
      );
    }
  }

  Future<bool> _savePolicyImmediate(PowerNetworkPolicyModel next) async {
    final prev = _policy;
    setState(() => _policy = next);
    try {
      await widget.bridge.updatePowerNetworkPolicy(
        wifiOnly: next.wifiOnly,
        allowedSsids: next.allowedSsids,
        requireCharging: next.requireCharging,
        maxKbps: next.maxKbps,
      );
      return true;
    } catch (err) {
      if (mounted) {
        setState(() => _policy = prev);
        NeuToast.error(context, LanVaultBridge.formatUserError(err));
      }
      return false;
    }
  }

  Future<void> _pickRoleMode() async {
    final neu = NeuTheme.of(context);
    final chosen = await showNeuDialog<RoleMode>(
      context: context,
      builder: (ctx) => NeuDialog(
        title: '节点运行模式',
        subtitle: '决定本机作为备份端、存储端还是双端同时运行',
        maxWidth: 380,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: <Widget>[
            for (final mode in RoleMode.values)
              ListTile(
                title: Text(mode.label),
                trailing: mode == _roleMode
                    ? Icon(Icons.check_rounded, color: neu.accent)
                    : null,
                onTap: () => Navigator.of(ctx).pop(mode),
              ),
          ],
        ),
      ),
    );
    if (chosen == null || chosen == _roleMode || !mounted) return;
    try {
      final applied = await widget.bridge.setRoleMode(chosen);
      if (!mounted) return;
      setState(() => _roleMode = applied);
      NeuToast.success(context, '已切换为 ${applied.label}');
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    }
  }

  Future<void> _editSpeedLimit() async {
    final ctrl = TextEditingController(
      text: _policy.maxKbps > 0 ? '${_policy.maxKbps}' : '',
    );
    final saved = await showNeuDialog<int>(
      context: context,
      builder: (ctx) => NeuDialog(
        title: '上传限速 (KB/s)',
        subtitle: '留空或填 0 表示不限速',
        maxWidth: 360,
        actions: <Widget>[
          NeuButton(label: '取消', onPressed: () => Navigator.of(ctx).pop()),
          NeuButton(
            label: '确定',
            tone: NeuButtonTone.primary,
            onPressed: () {
              final v = int.tryParse(ctrl.text.trim()) ?? 0;
              Navigator.of(ctx).pop(v < 0 ? 0 : v);
            },
          ),
        ],
        child: NeuInsetField(
          controller: ctrl,
          hint: '例如 10240（约 10 MB/s）',
          keyboardType: TextInputType.number,
          prefixIcon: Icons.speed_rounded,
        ),
      ),
    );
    unawaited(
        Future<void>.delayed(const Duration(milliseconds: 350), ctrl.dispose));
    if (saved == null || !mounted) return;
    await _savePolicyImmediate(
      PowerNetworkPolicyModel(
        wifiOnly: _policy.wifiOnly,
        allowedSsids: _policy.allowedSsids,
        requireCharging: _policy.requireCharging,
        maxKbps: saved,
      ),
    );
    if (mounted) {
      NeuToast.success(
        context,
        saved == 0 ? '已取消上传限速' : '已设置上传限速为 $saved KB/s',
      );
    }
  }

  Future<void> _openFailoverDirsDialog() async {
    final primaryCtrl =
        TextEditingController(text: _storageConfig?.primaryDir ?? '');
    final fallbackCtrl =
        TextEditingController(text: _storageConfig?.fallbackDir ?? '');

    final saved = await showNeuDialog<bool>(
      context: context,
      builder: (ctx) => NeuDialog(
        title: '主 / 备异盘存储目录',
        subtitle: '主盘空间不足时自动切入备用目录写入',
        maxWidth: 460,
        actions: <Widget>[
          NeuButton(label: '取消', onPressed: () => Navigator.of(ctx).pop(false)),
          NeuButton(
            label: '保存目录',
            tone: NeuButtonTone.primary,
            onPressed: () async {
              final primary = primaryCtrl.text.trim();
              final fallback = fallbackCtrl.text.trim();
              if (primary.isEmpty) {
                NeuToast.warning(ctx, '主存储目录不能为空');
                return;
              }
              try {
                final cfg = await widget.bridge.setBackupStorageConfig(
                  primaryDir: primary,
                  fallbackDir: fallback.isEmpty ? null : fallback,
                );
                if (mounted) setState(() => _storageConfig = cfg);
                if (ctx.mounted) Navigator.of(ctx).pop(true);
              } catch (err) {
                if (ctx.mounted) {
                  NeuToast.error(ctx, LanVaultBridge.formatUserError(err));
                }
              }
            },
          ),
        ],
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: <Widget>[
            Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: <Widget>[
                Expanded(
                  child: NeuInsetField(
                    controller: primaryCtrl,
                    label: '主存储目录',
                    hint: '选择主磁盘目录',
                    prefixIcon: Icons.folder_outlined,
                  ),
                ),
                const SizedBox(width: 8),
                NeuButton(
                  label: '浏览',
                  onPressed: () async {
                    final p = await widget.bridge.pickDirectory(
                      description: '选择主存储目录',
                    );
                    if (ctx.mounted && p != null && p.isNotEmpty) {
                      primaryCtrl.text = p;
                    }
                  },
                ),
              ],
            ),
            const SizedBox(height: 12),
            Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: <Widget>[
                Expanded(
                  child: NeuInsetField(
                    controller: fallbackCtrl,
                    label: '备用目录（可选，建议不同磁盘）',
                    hint: '选择备用磁盘目录',
                    prefixIcon: Icons.folder_copy_outlined,
                  ),
                ),
                const SizedBox(width: 8),
                NeuButton(
                  label: '浏览',
                  onPressed: () async {
                    final p = await widget.bridge.pickDirectory(
                      description: '选择备用存储目录',
                    );
                    if (ctx.mounted && p != null && p.isNotEmpty) {
                      fallbackCtrl.text = p;
                    }
                  },
                ),
              ],
            ),
          ],
        ),
      ),
    );
    unawaited(Future<void>.delayed(const Duration(milliseconds: 350), () {
      primaryCtrl.dispose();
      fallbackCtrl.dispose();
    }));
    if (saved == true && mounted) {
      NeuToast.success(context, '主 / 备存储目录已更新');
    }
  }

  Future<void> _toggleWebPassword(bool enabled) async {
    if (!enabled) {
      final confirmed = await showNeuDialog<bool>(
        context: context,
        builder: (ctx) => NeuDialog(
          title: '关闭 Web 访问密码',
          subtitle: '会立即停止本机存储服务，并清除已留档的密码；再次启动需要重新设置。',
          maxWidth: 420,
          actions: <Widget>[
            NeuButton(label: '取消', onPressed: () => Navigator.of(ctx).pop(false)),
            NeuButton(
              label: '关闭并停服',
              tone: NeuButtonTone.danger,
              onPressed: () => Navigator.of(ctx).pop(true),
            ),
          ],
          child: const SizedBox.shrink(),
        ),
      );
      if (confirmed != true || !mounted) return;
      try {
        await widget.bridge.clearWebPassword();
        if (!mounted) return;
        setState(() => _webPasswordEnabled = false);
        NeuToast.success(context, '已关闭 Web 密码保护并停止本机存储服务');
      } catch (err) {
        if (!mounted) return;
        // 失败时不改开关，让 UI 与真实状态保持一致。
        NeuToast.error(context, LanVaultBridge.formatUserError(err));
      }
      return;
    }
    final pwdCtrl = TextEditingController();
    final port = _lastServerConfig?.port ?? LanVaultBridge.defaultServerPort;
    final vaultPath = _lastServerConfig?.vaultPath ?? '';
    final quotaGb = _lastServerConfig?.quotaGb ?? 100;

    final confirmed = await showNeuDialog<String>(
      context: context,
      builder: (ctx) => NeuDialog(
        title: '设置 Web 远程访问密码',
        subtitle: '开启后，通过浏览器访问本机 Web 管理页需输入此密码（至少 8 位）',
        maxWidth: 400,
        actions: <Widget>[
          NeuButton(label: '取消', onPressed: () => Navigator.of(ctx).pop()),
          NeuButton(
            label: '启用密码',
            tone: NeuButtonTone.primary,
            onPressed: () {
              final p = pwdCtrl.text.trim();
              if (p.length < 8) {
                NeuToast.warning(ctx, '密码至少需要 8 位字符');
                return;
              }
              Navigator.of(ctx).pop(p);
            },
          ),
        ],
        child: NeuInsetField(
          controller: pwdCtrl,
          hint: '请输入至少 8 位密码',
          obscureText: true,
          prefixIcon: Icons.lock_outline_rounded,
        ),
      ),
    );
    unawaited(Future<void>.delayed(
        const Duration(milliseconds: 350), pwdCtrl.dispose));
    if (confirmed == null || !mounted) return;

    if (vaultPath.isNotEmpty) {
      try {
        await widget.bridge.startLocalServer(
          vaultPath: vaultPath,
          quotaGb: quotaGb,
          port: port,
          webPassword: confirmed,
        );
        if (!mounted) return;
        setState(() => _webPasswordEnabled = true);
        NeuToast.success(context, 'Web 远程访问密码已启用');
      } catch (err) {
        if (!mounted) return;
        NeuToast.error(context, LanVaultBridge.formatUserError(err));
      }
    } else {
      // 没有可用的最近配置时无法真正启动带密码的服务，绝不能只把开关点亮。
      NeuToast.warning(
        context,
        '还没有可用的存储目录配置，请先在"存储"页创建存储池后再启用密码',
      );
    }
  }

  Future<void> _triggerQuickScrub() async {
    setState(() => _scrubbing = true);
    try {
      final result = await widget.bridge.triggerBitRotScrub();
      if (!mounted) return;
      NeuToast.success(
        context,
        '校验完成：检查 ${result.checkedChunks} 块，发现 ${result.corruptedChunks} 个异常，已修复 ${result.healedChunks} 个',
      );
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    } finally {
      if (mounted) setState(() => _scrubbing = false);
    }
  }

  Future<void> _openKeyBackupDialog() async {
    final ctrl = TextEditingController();
    var exporting = false;
    await showNeuDialog<void>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (context, setDialogState) => NeuDialog(
          title: '主密钥加密备份',
          subtitle: '输入至少 12 位高强度口令导出密钥备份，换机时可用于离线恢复数据',
          maxWidth: 420,
          actions: <Widget>[
            NeuButton(
              label: '取消',
              onPressed: exporting ? null : () => Navigator.of(ctx).pop(),
            ),
            NeuButton(
              label: '导出并复制',
              icon: Icons.download_rounded,
              tone: NeuButtonTone.primary,
              loading: exporting,
              onPressed: exporting
                  ? null
                  : () async {
                      final pass = ctrl.text.trim();
                      if (pass.length < 12) {
                        NeuToast.warning(ctx, '备份口令至少需要 12 位');
                        return;
                      }
                      setDialogState(() => exporting = true);
                      try {
                        final json =
                            await widget.bridge.exportVaultKeyBackup(pass);
                        await Clipboard.setData(ClipboardData(text: json));
                        if (ctx.mounted) Navigator.of(ctx).pop();
                        if (mounted) {
                          NeuToast.success(
                            this.context,
                            '主密钥加密备份已复制到剪贴板，请妥善离线保存',
                          );
                        }
                      } catch (err) {
                        if (ctx.mounted) {
                          setDialogState(() => exporting = false);
                          NeuToast.error(
                            ctx,
                            LanVaultBridge.formatUserError(err),
                          );
                        }
                      }
                    },
            ),
          ],
          child: NeuInsetField(
            controller: ctrl,
            hint: '请输入至少 12 位高强度口令',
            obscureText: true,
            prefixIcon: Icons.vpn_key_outlined,
          ),
        ),
      ),
    );
    unawaited(
        Future<void>.delayed(const Duration(milliseconds: 350), ctrl.dispose));
  }

  Future<void> _restartServer() async {
    if (_restarting) return;
    setState(() => _restarting = true);
    try {
      final ok = await widget.bridge.executeGracefulRestart();
      if (!mounted) return;
      NeuToast.info(context, ok ? '后台服务已优雅重启' : '重启未完成，请稍后重试');
      await _loadAll();
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    } finally {
      if (mounted) setState(() => _restarting = false);
    }
  }

  Future<void> _runSelfDiag() async {
    if (_runningDiag) return;
    setState(() => _runningDiag = true);
    try {
      final report = await widget.bridge.runSelfDiagnostics();
      if (!mounted) return;
      final ok = report['ok'] != false;
      NeuToast.info(
        context,
        ok ? '探针自检通过：服务、存储与网络链路均正常' : '自检发现异常项，请查看诊断日志',
      );
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    } finally {
      if (mounted) setState(() => _runningDiag = false);
    }
  }

  Future<void> _openAllowedWifiDialog() async {
    final selected = List<String>.from(_policy.allowedSsids);
    var discovered = <String>[];
    var scanning = false;
    final manualCtrl = TextEditingController();

    await showNeuDialog<void>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (context, setDialogState) {
          return NeuDialog(
            title: '允许同步的家庭 Wi-Fi',
            subtitle: '留空表示不限制网络（包含网线与任意 Wi-Fi）',
            maxWidth: 440,
            actions: <Widget>[
              NeuButton(
                label: '清空限制',
                onPressed: () {
                  selected.clear();
                  unawaited(_savePolicyImmediate(PowerNetworkPolicyModel(
                    wifiOnly: _policy.wifiOnly,
                    allowedSsids: const <String>[],
                    requireCharging: _policy.requireCharging,
                    maxKbps: _policy.maxKbps,
                  )));
                  Navigator.of(ctx).pop();
                  NeuToast.info(this.context, '已恢复不限制同步网络');
                },
              ),
              NeuButton(
                label: '完成',
                tone: NeuButtonTone.primary,
                onPressed: () {
                  unawaited(_savePolicyImmediate(PowerNetworkPolicyModel(
                    wifiOnly: _policy.wifiOnly,
                    allowedSsids: List<String>.from(selected),
                    requireCharging: _policy.requireCharging,
                    maxKbps: _policy.maxKbps,
                  )));
                  Navigator.of(ctx).pop();
                  NeuToast.success(this.context, '家庭网络列表已生效');
                },
              ),
            ],
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                Row(
                  children: <Widget>[
                    Expanded(
                      child: NeuInsetField(
                        controller: manualCtrl,
                        hint: '输入 Wi-Fi 名称后添加',
                        prefixIcon: Icons.wifi_rounded,
                      ),
                    ),
                    const SizedBox(width: 8),
                    NeuButton(
                      label: '添加',
                      onPressed: () {
                        final name = manualCtrl.text.trim();
                        if (name.isNotEmpty && !selected.contains(name)) {
                          setDialogState(() {
                            selected.add(name);
                            manualCtrl.clear();
                          });
                        }
                      },
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                NeuButton(
                  label: scanning ? '正在扫描周边 Wi-Fi…' : '扫描周边 Wi-Fi',
                  icon: Icons.radar_rounded,
                  loading: scanning,
                  expanded: true,
                  onPressed: scanning
                      ? null
                      : () async {
                          setDialogState(() => scanning = true);
                          try {
                            final snap = await SystemEnvHelper.scanNetworks();
                            final found = <String>{
                              if (snap.currentSsid != null &&
                                  snap.currentSsid!.isNotEmpty)
                                snap.currentSsid!,
                              ...snap.availableSsids,
                            }.toList(growable: false);
                            if (ctx.mounted) {
                              setDialogState(() {
                                discovered = found;
                                scanning = false;
                              });
                            }
                          } catch (_) {
                            setDialogState(() => scanning = false);
                          }
                        },
                ),
                if (discovered.isNotEmpty) ...<Widget>[
                  const SizedBox(height: 10),
                  Wrap(
                    spacing: 6,
                    runSpacing: 6,
                    children: <Widget>[
                      for (final ssid in discovered)
                        ActionChip(
                          label: Text(ssid),
                          onPressed: () {
                            if (!selected.contains(ssid)) {
                              setDialogState(() => selected.add(ssid));
                            }
                          },
                        ),
                    ],
                  ),
                ],
                if (selected.isNotEmpty) ...<Widget>[
                  const SizedBox(height: 12),
                  Wrap(
                    spacing: 6,
                    runSpacing: 6,
                    children: <Widget>[
                      for (final ssid in selected)
                        InputChip(
                          label: Text(ssid),
                          onDeleted: () =>
                              setDialogState(() => selected.remove(ssid)),
                        ),
                    ],
                  ),
                ],
              ],
            ),
          );
        },
      ),
    );
    unawaited(Future<void>.delayed(
        const Duration(milliseconds: 350), manualCtrl.dispose));
  }

  Future<void> _openDebugLogsDialog() async {
    var logs = await widget.bridge.getDebugLogs(limit: 80);
    if (!mounted) return;
    await showNeuDialog<void>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (context, setDialogState) => NeuDialog(
          title: '运行与调试日志',
          subtitle: '最近 ${logs.length} 条记录',
          maxWidth: 560,
          actions: <Widget>[
            NeuButton(
              label: '清空日志',
              tone: NeuButtonTone.danger,
              onPressed: () async {
                await widget.bridge.clearDebugLogs();
                if (ctx.mounted) {
                  setDialogState(() => logs = const <DebugLogEntryModel>[]);
                }
                if (mounted) {
                  NeuToast.info(this.context, '调试日志已清空');
                }
              },
            ),
            NeuButton(
              label: '关闭',
              tone: NeuButtonTone.primary,
              onPressed: () => Navigator.of(ctx).pop(),
            ),
          ],
          child: SizedBox(
            height: 300,
            child: logs.isEmpty
                ? const Center(child: Text('暂无调试日志记录'))
                : ListView.separated(
                    itemCount: logs.length,
                    separatorBuilder: (_, __) => const Divider(height: 1),
                    itemBuilder: (_, idx) {
                      final e = logs[idx];
                      return Padding(
                        padding: const EdgeInsets.symmetric(vertical: 6),
                        child: Text(
                          '[${e.level}] ${e.source}: ${e.message}',
                          style: Theme.of(context).textTheme.bodySmall,
                        ),
                      );
                    },
                  ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final isWide = MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    if (!isWide) {
      // 手机端：对标参考图，单一纵向分组白卡列表，零顶部两排药丸
      return SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(16, 6, 16, 28),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            if (_loadFailures.isNotEmpty) _buildLoadFailureBanner(),
            _buildSectionHeader(context, '常规与节点'),
            _buildGeneralGroup(context),
            _buildSectionHeader(context, '外观与动效'),
            _SettingsGroupCard(
                children: <Widget>[_buildAppearanceGroup(context)]),
            _buildSectionHeader(context, '备份与容灾'),
            _buildBackupGroup(context),
            _buildSectionHeader(context, 'Web 访问与安全'),
            _buildWebSecurityGroup(context),
            if (Platform.isAndroid) ...<Widget>[
              _buildSectionHeader(context, '权限与后台保活'),
              _buildAndroidPermissionsGroup(context),
            ],
            _buildSectionHeader(context, '服务与诊断'),
            _buildServiceDiagGroup(context),
            _buildSectionHeader(context, '关于'),
            _buildAboutGroup(context),
          ],
        ),
      );
    }

    // 桌面端：左侧 5 项清爽分类 + 右侧最大宽度 720px 分组卡
    return Padding(
      padding: const EdgeInsets.all(NeuSpace.lg),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          SizedBox(
            width: 196,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                Text(
                  '设置',
                  style: Theme.of(context)
                      .textTheme
                      .headlineSmall
                      ?.copyWith(fontWeight: FontWeight.w800),
                ),
                const SizedBox(height: 12),
                Container(
                  padding: const EdgeInsets.all(8),
                  decoration: BoxDecoration(
                    color: neu.surface,
                    borderRadius: const BorderRadius.all(Radius.circular(16)),
                    border: Border.all(color: neu.hairlineBorder),
                    boxShadow: neu.extrudedShadows,
                  ),
                  child: Column(
                    children: <Widget>[
                      for (final entry
                          in const <(_SettingsSection, String, IconData)>[
                        (_SettingsSection.general, '常规与节点', Icons.tune_rounded),
                        (
                          _SettingsSection.appearance,
                          '外观与动效',
                          Icons.palette_outlined
                        ),
                        (
                          _SettingsSection.backup,
                          '备份与容灾',
                          Icons.cloud_sync_outlined
                        ),
                        (
                          _SettingsSection.webSecurity,
                          'Web 访问与安全',
                          Icons.shield_outlined
                        ),
                        (
                          _SettingsSection.serviceDiag,
                          '服务与诊断',
                          Icons.dns_outlined
                        ),
                        (
                          _SettingsSection.about,
                          '关于',
                          Icons.info_outline_rounded
                        ),
                      ]) ...<Widget>[
                        NavRailItem(
                          label: entry.$2,
                          icon: entry.$3,
                          selectedIcon: entry.$3,
                          selected: _desktopSection == entry.$1,
                          onTap: () =>
                              setState(() => _desktopSection = entry.$1),
                        ),
                        if (entry.$1 != _SettingsSection.about)
                          const SizedBox(height: 4),
                      ],
                    ],
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: NeuSpace.lg),
          Expanded(
            child: SingleChildScrollView(
              child: Align(
                alignment: Alignment.topLeft,
                child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 720),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: <Widget>[
                      if (_loadFailures.isNotEmpty) _buildLoadFailureBanner(),
                      _buildDesktopSectionBody(context),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildDesktopSectionBody(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        switch (_desktopSection) {
          _SettingsSection.general =>
            _buildSectionHeader(context, '常规与节点', topPadding: 0),
          _SettingsSection.appearance =>
            _buildSectionHeader(context, '外观与动效', topPadding: 0),
          _SettingsSection.backup =>
            _buildSectionHeader(context, '备份与容灾', topPadding: 0),
          _SettingsSection.webSecurity =>
            _buildSectionHeader(context, 'Web 访问与安全', topPadding: 0),
          _SettingsSection.serviceDiag =>
            _buildSectionHeader(context, '服务与诊断', topPadding: 0),
          _SettingsSection.about =>
            _buildSectionHeader(context, '关于', topPadding: 0),
        },
        switch (_desktopSection) {
          _SettingsSection.general => _buildGeneralGroup(context),
          _SettingsSection.appearance => _SettingsGroupCard(
              children: <Widget>[_buildAppearanceGroup(context)]),
          _SettingsSection.backup => _buildBackupGroup(context),
          _SettingsSection.webSecurity => _buildWebSecurityGroup(context),
          _SettingsSection.serviceDiag => _buildServiceDiagGroup(context),
          _SettingsSection.about => _buildAboutGroup(context),
        },
      ],
    );
  }

  Widget _buildSectionHeader(
    BuildContext context,
    String title, {
    double topPadding = 18,
  }) {
    final neu = NeuTheme.of(context);
    return Padding(
      padding: EdgeInsets.fromLTRB(6, topPadding, 6, 8),
      child: Text(
        title,
        style: Theme.of(context).textTheme.labelLarge?.copyWith(
              color: neu.accent,
              fontWeight: FontWeight.w700,
              fontSize: 13,
            ),
      ),
    );
  }

  Widget _buildLoadFailureBanner() {
    return Padding(
      padding: const EdgeInsets.only(bottom: 12),
      child: NeuCard(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        backgroundColor: NeuTheme.of(context).folderGoldSoft,
        child: Row(
          children: <Widget>[
            Icon(
              Icons.warning_amber_rounded,
              size: 18,
              color: NeuTheme.of(context).warning,
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Text(
                '部分配置读取失败（${_loadFailures.join('、')}），请勿在重试前覆盖设置。',
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ),
            const SizedBox(width: 8),
            NeuButton(
              label: '重试',
              onPressed: () => unawaited(_loadAll()),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildGeneralGroup(BuildContext context) {
    return _SettingsGroupCard(
      children: <Widget>[
        _SettingsValueRow(
          title: '节点运行模式',
          subtitle: '决定本机作为备份端、存储端或双端运行',
          value: _roleMode.label,
          icon: Icons.unfold_more_rounded,
          onTap: _pickRoleMode,
        ),
        _SettingsSwitchRow(
          title: '断线自动重连',
          subtitle: '局域网恢复后自动重连已配对设备',
          value: _autoReconnect,
          onChanged: (v) async {
            final prev = _autoReconnect;
            setState(() => _autoReconnect = v);
            try {
              await widget.bridge.enableAutoReconnect(v);
            } catch (err) {
              if (!mounted) return;
              setState(() => _autoReconnect = prev);
              NeuToast.error(this.context, LanVaultBridge.formatUserError(err));
            }
          },
        ),
        _SettingsSwitchRow(
          title: '随软件自动运行本地存储',
          subtitle: '启动后后台常驻，无需每次手动开启',
          value: _autostartServer,
          onChanged: (v) async {
            final prev = _autostartServer;
            setState(() => _autostartServer = v);
            try {
              await widget.bridge.setAutostartServer(v);
            } catch (err) {
              if (!mounted) return;
              setState(() => _autostartServer = prev);
              NeuToast.error(this.context, LanVaultBridge.formatUserError(err));
            }
          },
        ),
      ],
    );
  }

  Widget _buildBackupGroup(BuildContext context) {
    final speedText = _policy.maxKbps > 0 ? '${_policy.maxKbps} KB/s' : '不限速';
    final primaryText = (_storageConfig?.primaryDir.isNotEmpty ?? false)
        ? _storageConfig!.primaryDir
        : '默认目录';

    return _SettingsGroupCard(
      children: <Widget>[
        _SettingsSwitchRow(
          title: '仅 Wi-Fi 下执行同步',
          subtitle: '避免消耗移动蜂窝流量',
          value: _policy.wifiOnly,
          onChanged: (v) => unawaited(
            _savePolicyImmediate(
              PowerNetworkPolicyModel(
                wifiOnly: v,
                allowedSsids: _policy.allowedSsids,
                requireCharging: _policy.requireCharging,
                maxKbps: _policy.maxKbps,
              ),
            ),
          ),
        ),
        _SettingsSwitchRow(
          title: '仅充电时全速上传',
          subtitle: '未充电时降低后台耗电',
          value: _policy.requireCharging,
          onChanged: (v) => unawaited(
            _savePolicyImmediate(
              PowerNetworkPolicyModel(
                wifiOnly: _policy.wifiOnly,
                allowedSsids: _policy.allowedSsids,
                requireCharging: v,
                maxKbps: _policy.maxKbps,
              ),
            ),
          ),
        ),
        _SettingsValueRow(
          title: '允许同步的家庭 Wi-Fi',
          value: _policy.allowedSsids.isEmpty
              ? '不限制'
              : '${_policy.allowedSsids.length} 个网络',
          icon: Icons.chevron_right_rounded,
          onTap: _openAllowedWifiDialog,
        ),
        _SettingsValueRow(
          title: '上传限速',
          value: speedText,
          icon: Icons.edit_outlined,
          onTap: _editSpeedLimit,
        ),
        _SettingsValueRow(
          title: '主 / 备异盘存储目录',
          subtitle: primaryText,
          value: '配置',
          icon: Icons.chevron_right_rounded,
          onTap: _openFailoverDirsDialog,
        ),
      ],
    );
  }

  Widget _buildWebSecurityGroup(BuildContext context) {
    final port = _overview?.serverPort ??
        _lastServerConfig?.port ??
        LanVaultBridge.defaultServerPort;
    return _SettingsGroupCard(
      children: <Widget>[
        _SettingsValueRow(
          title: '服务端口',
          value: '$port',
          icon: Icons.lan_outlined,
          onTap: null,
        ),
        _SettingsSwitchRow(
          title: 'Web 访问密码保护',
          subtitle: '开启后，通过浏览器连接 Web 管理页需输入密码',
          value: _webPasswordEnabled,
          onChanged: (v) => unawaited(_toggleWebPassword(v)),
        ),
        _SettingsSwitchRow(
          title: '数据完整性定期检查',
          subtitle: '后台定期校验备份数据块并利用副本自愈',
          value: _bitRotAutoScrub,
          trailing: NeuButton(
            label: _scrubbing ? '校验中…' : '立即校验',
            icon: Icons.verified_outlined,
            loading: _scrubbing,
            onPressed: _scrubbing ? null : _triggerQuickScrub,
          ),
          onChanged: (v) async {
            final prev = _bitRotAutoScrub;
            setState(() => _bitRotAutoScrub = v);
            try {
              await widget.bridge.setBitRotAutoScrub(v);
            } catch (err) {
              if (!mounted) return;
              setState(() => _bitRotAutoScrub = prev);
              NeuToast.error(this.context, LanVaultBridge.formatUserError(err));
            }
          },
        ),
        _SettingsValueRow(
          title: '主密钥加密备份',
          subtitle: '导出加密主密钥，换机时可离线恢复数据',
          value: '导出',
          icon: Icons.chevron_right_rounded,
          onTap: _openKeyBackupDialog,
        ),
      ],
    );
  }

  Widget _buildAndroidPermissionsGroup(BuildContext context) {
    return _SettingsGroupCard(
      children: <Widget>[
        _SettingsValueRow(
          title: '所有文件访问权限',
          subtitle: '用于读写自定义目录与存储池文件',
          value: '去授权',
          icon: Icons.chevron_right_rounded,
          onTap: () async {
            final ok = await widget.bridge.requestStoragePermission();
            if (!mounted) return;
            NeuToast.info(
              this.context,
              ok ? '已获得所有文件访问权限' : '请在系统设置中授予所有文件访问权限',
            );
          },
        ),
        _SettingsValueRow(
          title: '通知权限',
          subtitle: '保持后台服务常驻与传输进度提醒',
          value: '去授权',
          icon: Icons.chevron_right_rounded,
          onTap: () async {
            final ok = await widget.bridge.requestNotificationPermission();
            if (!mounted) return;
            NeuToast.info(
              this.context,
              ok ? '已获得通知权限' : '请在系统设置中允许通知权限',
            );
          },
        ),
        _SettingsValueRow(
          title: '电池优化白名单',
          subtitle: '防止锁屏后系统休眠中断局域网同步',
          value: '去设置',
          icon: Icons.chevron_right_rounded,
          onTap: () async {
            final ok = await widget.bridge.requestBatteryExemption();
            if (!mounted) return;
            NeuToast.info(
              this.context,
              ok ? '已加入电池优化白名单' : '请在系统电池设置中设为无限制',
            );
          },
        ),
        _SettingsValueRow(
          title: 'Wi-Fi 定位权限',
          subtitle: '用于识别当前连接的家庭 Wi-Fi 名称',
          value: '去授权',
          icon: Icons.chevron_right_rounded,
          onTap: () async {
            final ok = await widget.bridge.requestLocationPermission();
            if (!mounted) return;
            NeuToast.info(
              this.context,
              ok ? '已获得 Wi-Fi 定位权限' : '请在系统设置中授予位置权限',
            );
          },
        ),
      ],
    );
  }

  Widget _buildServiceDiagGroup(BuildContext context) {
    final running = _overview?.serverRunning ?? false;
    final lanIp = _overview?.lanIp ?? '未获取';

    return _SettingsGroupCard(
      children: <Widget>[
        _SettingsValueRow(
          title: '本机局域网地址',
          value: lanIp,
          icon: Icons.wifi_rounded,
          onTap: null,
        ),
        _SettingsValueRow(
          title: '重启后台服务',
          subtitle: running ? '当前运行中，异常时可优雅重启' : '当前未启动，点击尝试拉起',
          value: _restarting ? '重启中…' : '重启',
          icon: Icons.restart_alt_rounded,
          onTap: _restarting ? null : _restartServer,
        ),
        _SettingsSwitchRow(
          title: 'Debug 调试日志',
          subtitle: '排查问题时开启，记录详细运行日志',
          value: _debugMode,
          onChanged: (v) async {
            final prev = _debugMode;
            setState(() => _debugMode = v);
            try {
              await widget.bridge.setDebugMode(v);
            } catch (err) {
              if (!mounted) return;
              setState(() => _debugMode = prev);
              NeuToast.error(this.context, LanVaultBridge.formatUserError(err));
            }
          },
        ),
        _SettingsValueRow(
          title: '查看调试日志',
          subtitle: '查看或清空近期运行与探针日志',
          value: '查看',
          icon: Icons.chevron_right_rounded,
          onTap: _openDebugLogsDialog,
        ),
        _SettingsValueRow(
          title: '一键链路自检',
          subtitle: '检测本地存储、网络端口与服务健康状态',
          value: _runningDiag ? '检测中…' : '运行',
          icon: Icons.chevron_right_rounded,
          onTap: _runningDiag ? null : _runSelfDiag,
        ),
      ],
    );
  }

  Widget _buildAboutGroup(BuildContext context) {
    return _SettingsGroupCard(
      children: <Widget>[
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
          child: Row(
            children: <Widget>[
              const BrandLogoMark(size: 40),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Text(
                      'EveryoneNAS',
                      style: Theme.of(context).textTheme.titleMedium?.copyWith(
                            fontWeight: FontWeight.w800,
                          ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      'v${LanVaultBridge.appVersion} · 局域网私有云与跨端备份',
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        const _SettingsValueRow(
          title: '支持平台',
          value: 'Windows & Android',
          icon: Icons.devices_rounded,
          onTap: null,
        ),
        const _SettingsValueRow(
          title: '安全架构',
          value: '端到端零知识加密',
          icon: Icons.verified_user_outlined,
          onTap: null,
        ),
      ],
    );
  }
}

class _SettingsGroupCard extends StatelessWidget {
  const _SettingsGroupCard({required this.children});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    return NeuCard(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          for (var i = 0; i < children.length; i++) ...<Widget>[
            children[i],
            if (i < children.length - 1)
              Padding(
                padding: EdgeInsets.symmetric(horizontal: 16),
                child: Divider(height: 1, color: neu.dividerSubtle),
              ),
          ],
        ],
      ),
    );
  }
}

class _SettingsSwitchRow extends StatelessWidget {
  const _SettingsSwitchRow({
    required this.title,
    this.subtitle,
    required this.value,
    required this.onChanged,
    this.trailing,
  });

  final String title;
  final String? subtitle;
  final bool value;
  final ValueChanged<bool> onChanged;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    // 标题 + 开关合并成一个可朗读、可键盘操作的节点（NeuSwitch），
    // 行内还可能有额外动作按钮（如「立即校验」），所以保留外层 Row。
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16),
      child: Row(
        children: <Widget>[
          Expanded(
            child: NeuSwitch(
              title: title,
              subtitle: subtitle,
              value: value,
              onChanged: onChanged,
              padding: const EdgeInsets.symmetric(vertical: 12),
            ),
          ),
          if (trailing != null) ...<Widget>[
            const SizedBox(width: 10),
            trailing!,
          ],
        ],
      ),
    );
  }
}

class _SettingsValueRow extends StatelessWidget {
  const _SettingsValueRow({
    required this.title,
    this.subtitle,
    required this.value,
    required this.icon,
    required this.onTap,
  });

  final String title;
  final String? subtitle;
  final String value;
  final IconData icon;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    return InkWell(
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 13),
        child: Row(
          children: <Widget>[
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: <Widget>[
                  Text(
                    title,
                    style: Theme.of(context).textTheme.titleSmall?.copyWith(
                          fontWeight: FontWeight.w600,
                        ),
                  ),
                  if (subtitle != null && subtitle!.isNotEmpty) ...<Widget>[
                    const SizedBox(height: 3),
                    Text(
                      subtitle!,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                            color: neu.textSecondary,
                          ),
                    ),
                  ],
                ],
              ),
            ),
            const SizedBox(width: 12),
            ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 160),
              child: Text(
                value,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: Theme.of(context).textTheme.bodySmall?.copyWith(
                      color: neu.textSecondary,
                      fontWeight: FontWeight.w500,
                    ),
              ),
            ),
            if (onTap != null) ...<Widget>[
              const SizedBox(width: 4),
              Icon(icon, size: 17, color: neu.textSecondary),
            ],
          ],
        ),
      ),
    );
  }
}

class _SkinMarketCard extends StatelessWidget {
  const _SkinMarketCard({
    required this.skin,
    required this.selected,
    required this.onTap,
  });

  final SkinDefinition skin;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    return Semantics(
      button: true,
      selected: selected,
      label: '应用${skin.name}皮肤',
      child: InkWell(
        onTap: onTap,
        borderRadius:
            const BorderRadius.all(Radius.circular(NeuRadius.control)),
        child: AnimatedContainer(
          duration: NeuMotion.resolveState(context),
          curve: NeuMotion.resolveStateCurve(context),
          width: 132,
          padding: const EdgeInsets.all(10),
          decoration: BoxDecoration(
            color: selected ? neu.accentSoft : neu.surface,
            borderRadius:
                const BorderRadius.all(Radius.circular(NeuRadius.control)),
            border: Border.all(
              color: selected ? neu.accent : neu.hairlineBorder,
              width: selected ? 2 : 1,
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Row(
                children: <Widget>[
                  for (final swatch in skin.swatches) ...<Widget>[
                    Container(
                      width: 24,
                      height: 24,
                      decoration: BoxDecoration(
                        color: swatch,
                        shape: BoxShape.circle,
                        border: Border.all(color: neu.hairlineBorder),
                      ),
                    ),
                    const SizedBox(width: 4),
                  ],
                  const Spacer(),
                  if (selected)
                    Icon(Icons.check_circle_rounded,
                        size: 18, color: neu.accent),
                ],
              ),
              const SizedBox(height: 8),
              Text(
                skin.name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: Theme.of(context).textTheme.titleSmall,
              ),
            ],
          ),
        ),
      ),
    );
  }
}
