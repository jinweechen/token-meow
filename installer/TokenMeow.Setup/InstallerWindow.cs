using System;
using System.ComponentModel;
using System.IO;
using System.Reflection;
using System.Text;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Effects;
using System.Windows.Media.Imaging;
using System.Windows.Shapes;
using DrawingPath = System.Windows.Shapes.Path;

namespace TokenMeow.Setup
{
    // The host owns installation and launch. This window only renders their results.
    public sealed class InstallerWindow : Window
    {
        private static readonly Brush Ink = BrushOf(43, 57, 47);
        private static readonly Brush Muted = BrushOf(113, 124, 111);
        private static readonly Brush Green = BrushOf(91, 121, 88);
        private static readonly Brush GreenHover = BrushOf(75, 105, 73);
        private static readonly Brush Cream = BrushOf(252, 251, 246);
        private static readonly Brush Line = BrushOf(226, 230, 218);
        private static readonly Brush Pale = BrushOf(239, 243, 232);
        private readonly InstallerEnvironment environment;
        private readonly ContentControl pageHost = new ContentControl();
        private readonly ContentControl footerHost = new ContentControl();
        private readonly TextBlock chromeStatus = new TextBlock();
        private readonly Button closeButton;
        private TextBox directoryBox;
        private CheckBox shortcutBox;
        private CheckBox launchBox;
        private TextBlock pathError;
        private TextBlock progressTitle;
        private TextBlock progressDetail;
        private TextBlock progressFootnote;
        private Border[] stepMarkers;
        private TextBlock[] stepNumbers;
        private TextBlock[] stepLabels;
        private Button primaryButton;
        private Border launchError;
        private string directory;
        private bool createShortcuts = true;
        private bool advancedExpanded;
        private bool installing;
        private bool launching;
        private bool succeeded;
        private string installError;
        private InstallResult installedResult;
        private int currentStep;

        public event Action<InstallOptions> InstallRequested;
        public event Action LaunchRequested;

        public InstallerWindow(InstallerEnvironment env)
        {
            if (env == null) throw new ArgumentNullException("env");
            environment = env;
            directory = env.DefaultDirectory ?? String.Empty;
            Title = BuildInfo.ProductName + " 安装";
            Width = 960;
            Height = 650;
            WindowStartupLocation = WindowStartupLocation.CenterScreen;
            WindowStyle = WindowStyle.None;
            ResizeMode = ResizeMode.CanMinimize;
            AllowsTransparency = true;
            Background = Brushes.Transparent;
            FontFamily = new FontFamily("Microsoft YaHei UI, Segoe UI");
            Foreground = Ink;
            UseLayoutRounding = true;
            SnapsToDevicePixels = true;

            Border shell = new Border {
                Margin = new Thickness(12), Background = Cream,
                CornerRadius = new CornerRadius(24), BorderThickness = new Thickness(1),
                BorderBrush = BrushOf(226, 230, 217),
                Effect = new DropShadowEffect { BlurRadius = 20, ShadowDepth = 3, Opacity = 0.14, Color = Colors.Black }
            };
            Grid columns = new Grid();
            columns.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(310) });
            columns.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            shell.Child = columns;
            columns.Children.Add(BuildBrand());

            Grid right = new Grid { Margin = new Thickness(34, 10, 30, 24) };
            right.RowDefinitions.Add(new RowDefinition { Height = new GridLength(36) });
            right.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
            right.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            Grid.SetColumn(right, 1);
            columns.Children.Add(right);

            Grid chrome = new Grid { Background = Brushes.Transparent };
            chrome.MouseLeftButtonDown += DragChrome;
            chromeStatus.FontSize = 11;
            chromeStatus.Foreground = Muted;
            chromeStatus.VerticalAlignment = VerticalAlignment.Center;
            chromeStatus.Text = env.Preview ? "界面预览 · 不执行安装" : "为当前 Windows 用户安装";
            chrome.Children.Add(chromeStatus);
            StackPanel windowButtons = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right };
            Button minimize = ChromeButton("—", "最小化");
            minimize.Click += delegate { WindowState = WindowState.Minimized; };
            closeButton = ChromeButton("×", "关闭安装器");
            closeButton.Click += delegate { Close(); };
            windowButtons.Children.Add(minimize);
            windowButtons.Children.Add(closeButton);
            chrome.Children.Add(windowButtons);
            right.Children.Add(chrome);
            Grid.SetRow(pageHost, 1);
            pageHost.HorizontalContentAlignment = HorizontalAlignment.Stretch;
            pageHost.VerticalContentAlignment = VerticalAlignment.Stretch;
            right.Children.Add(pageHost);
            Grid.SetRow(footerHost, 2);
            footerHost.HorizontalContentAlignment = HorizontalAlignment.Stretch;
            right.Children.Add(footerHost);
            Content = shell;
            KeyboardNavigation.SetTabNavigation(shell, KeyboardNavigationMode.Cycle);
            PreviewKeyDown += delegate(object sender, KeyEventArgs args) {
                if (args.Key == Key.Escape) { Close(); args.Handled = true; }
            };
            Loaded += delegate {
                Rect work = SystemParameters.WorkArea;
                Width = Math.Min(960, Math.Max(600, work.Width - 24));
                Height = Math.Min(650, Math.Max(460, work.Height - 24));
                Left = work.Left + (work.Width - Width) / 2;
                Top = work.Top + (work.Height - Height) / 2;
                FocusPrimary();
            };
            RenderWelcome(false);
        }

        public void ShowProgress(InstallProgress progress)
        {
            if (progress == null) throw new ArgumentNullException("progress");
            OnUi(delegate {
                // A late worker notification cannot replace the successful result.
                if (succeeded) return;
                if (progressTitle == null) RenderProgress();
                SetInstalling(true);
                currentStep = Math.Max(0, Math.Min(2, progress.Step));
                progressTitle.Text = Clean(progress.Title, 180);
                progressDetail.Text = Clean(progress.Detail, 2000);
                if (String.IsNullOrWhiteSpace(progressTitle.Text)) progressTitle.Text = StageName(currentStep);
                UpdateSteps();
            });
        }

        public void ShowSuccess(InstallResult result)
        {
            if (result == null) throw new ArgumentNullException("result");
            OnUi(delegate {
                installedResult = result;
                succeeded = true;
                launching = false;
                SetInstalling(false);
                RenderSuccess();
                FocusPrimary();
            });
        }

        public void ShowError(string error)
        {
            OnUi(delegate {
                if (succeeded) { ShowLaunchError(error); return; }
                installError = Clean(error, 6000);
                if (String.IsNullOrWhiteSpace(installError)) installError = "安装暂未完成，请重试或更改安装位置。";
                SetInstalling(false);
                RenderError();
                FocusPrimary();
            });
        }

        public void ShowLaunchError(string error)
        {
            OnUi(delegate {
                if (!succeeded) return;
                launching = false;
                if (launchError != null) {
                    bool preview = environment.Preview || (installedResult != null && installedResult.Preview);
                    launchError.Child = Text((preview ? "预览 · 启动失败提示。" : "应用已安装，暂时未能打开。") + Environment.NewLine + Clean(error, 2500), 12, BrushOf(143, 81, 63));
                    launchError.Visibility = Visibility.Visible;
                }
                if (primaryButton != null) { primaryButton.IsEnabled = true; UpdateCompletionButton(); }
                if (launchBox != null) launchBox.IsEnabled = true;
                FocusPrimary();
            });
        }

        protected override void OnClosing(CancelEventArgs e)
        {
            base.OnClosing(e);
            if (installing && !environment.Preview) {
                e.Cancel = true;
                chromeStatus.Text = "安装正在进行，请等待当前步骤结束。";
                if (progressFootnote != null) progressFootnote.Text = "安装进行中，请等待完成。此窗口不会强行终止安装进程。";
            }
        }

        private UIElement BuildBrand()
        {
            Border left = new Border {
                Background = new LinearGradientBrush(BrushOf(232, 239, 220).Color, BrushOf(244, 244, 232).Color, 90),
                CornerRadius = new CornerRadius(23, 0, 0, 23), Padding = new Thickness(28, 31, 24, 25)
            };
            Grid grid = new Grid();
            grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            grid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
            grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            left.Child = grid;
            left.MouseLeftButtonDown += DragChrome;
            StackPanel brand = new StackPanel();
            TextBlock name = Text(BuildInfo.ProductName, 24, Ink);
            name.FontWeight = FontWeights.SemiBold;
            brand.Children.Add(name);
            brand.Children.Add(Text("一只懂得安静陪伴的桌宠", 12, Muted, new Thickness(0, 7, 0, 0)));
            Border version = new Border { Background = BrushOf(252, 253, 247), CornerRadius = new CornerRadius(8), Padding = new Thickness(9, 4, 9, 4), HorizontalAlignment = HorizontalAlignment.Left, Margin = new Thickness(0, 14, 0, 0) };
            version.Child = Text((environment.Preview ? "PREVIEW  ·  " : "WINDOWS  ·  ") + "v" + BuildInfo.ProductVersion, 10, Green);
            brand.Children.Add(version);
            grid.Children.Add(brand);

            Grid visual = new Grid { Margin = new Thickness(-8, 7, -8, 3) };
            Grid.SetRow(visual, 1);
            Ellipse halo = new Ellipse { Width = 245, Height = 245, Fill = BrushOf(249, 250, 239), Opacity = 0.8, VerticalAlignment = VerticalAlignment.Center };
            visual.Children.Add(halo);
            Ellipse ground = new Ellipse { Width = 150, Height = 17, Fill = BrushOf(190, 205, 178), Opacity = 0.24, VerticalAlignment = VerticalAlignment.Bottom, Margin = new Thickness(0, 0, 0, 4) };
            visual.Children.Add(ground);
            Image hero = new Image { Stretch = Stretch.Uniform, Margin = new Thickness(0, 0, 0, 8), IsHitTestVisible = false };
            AutomationProperties.SetName(hero, "桌宠小柚，白发绿裙的猫娘角色");
            try {
                using (Stream stream = Assembly.GetExecutingAssembly().GetManifestResourceStream(BuildInfo.HeroResourceName)) {
                    if (stream == null) throw new InvalidDataException("Missing embedded hero image");
                    BitmapImage bitmap = new BitmapImage();
                    bitmap.BeginInit();
                    bitmap.CacheOption = BitmapCacheOption.OnLoad;
                    bitmap.DecodePixelHeight = 1000;
                    bitmap.StreamSource = stream;
                    bitmap.EndInit();
                    bitmap.Freeze();
                    hero.Source = bitmap;
                }
                visual.Children.Add(hero);
            } catch (Exception) {
                TextBlock fallback = Text("小柚", 40, Green);
                fallback.HorizontalAlignment = HorizontalAlignment.Center;
                fallback.VerticalAlignment = VerticalAlignment.Center;
                visual.Children.Add(fallback);
            }
            grid.Children.Add(visual);
            StackPanel caption = new StackPanel();
            TextBlock hello = Text("你好，我是小柚。", 18, Green);
            hello.FontWeight = FontWeights.SemiBold;
            caption.Children.Add(hello);
            caption.Children.Add(Text("把专注留给重要的事，\n让我陪你慢慢完成。", 12, Muted, new Thickness(0, 8, 0, 0)));
            Grid.SetRow(caption, 2);
            grid.Children.Add(caption);
            return left;
        }

        private void RenderWelcome(bool editPath)
        {
            progressTitle = null;
            succeeded = false;
            StackPanel page = Page("欢迎小柚来到你的桌面", "轻轻装好，让每一次专注都有迹可循。");
            page.Children.Add(Feature("M3,17 L3,5 M3,17 L21,17 M8,14 L8,10 M13,14 L13,6 M18,14 L18,3", "本机 Codex，只读用量账本", "查看本地用量与响应记录，回顾你的专注。"));
            page.Children.Add(Feature("M4,5 L20,5 L20,20 L4,20 Z M8,2 L8,8 M16,2 L16,8 M8,12 L16,12 M8,16 L13,16", "会话、工具与收件箱提醒", "把工作线索收在一起，待处理事项不遗漏。"));
            page.Children.Add(Feature("M5,8 L3,2 L9,5 Q12,3 15,5 L21,2 L19,8 Q23,19 12,21 Q1,19 5,8 M8,12 L8,13 M16,12 L16,13 M10,17 Q12,19 14,17", "安静的桌宠陪伴", "在桌面陪你工作，也支持减少动画与免打扰。"));
            Border options = new Border { Background = Brushes.White, BorderBrush = Line, BorderThickness = new Thickness(1), CornerRadius = new CornerRadius(12), Padding = new Thickness(14, 11, 14, 11), Margin = new Thickness(0, 17, 0, 0) };
            StackPanel optionStack = new StackPanel();
            DockPanel pathRow = new DockPanel();
            Button change = MakeButton("更改", false);
            change.Padding = new Thickness(10, 5, 10, 5);
            change.FontSize = 12;
            change.MinHeight = 30;
            change.VerticalAlignment = VerticalAlignment.Center;
            DockPanel.SetDock(change, Dock.Right);
            pathRow.Children.Add(change);
            StackPanel location = new StackPanel { Margin = new Thickness(0, 0, 10, 0) };
            location.Children.Add(Text("安装位置", 11, Muted));
            TextBlock pathSummary = Text(directory, 11, Ink, new Thickness(0, 4, 0, 0));
            pathSummary.TextWrapping = TextWrapping.NoWrap;
            pathSummary.TextTrimming = TextTrimming.CharacterEllipsis;
            pathSummary.ToolTip = directory;
            location.Children.Add(pathSummary);
            pathRow.Children.Add(location);
            optionStack.Children.Add(pathRow);
            StackPanel editor = new StackPanel { Visibility = editPath || advancedExpanded ? Visibility.Visible : Visibility.Collapsed, Margin = new Thickness(0, 12, 0, 0) };
            directoryBox = new TextBox { Text = directory, FontSize = 12, Padding = new Thickness(10, 8, 10, 8), BorderBrush = Line, BorderThickness = new Thickness(1), Background = Cream, Foreground = Ink, MinHeight = 35 };
            AutomationProperties.SetName(directoryBox, "安装目录，可直接输入完整路径");
            directoryBox.TextChanged += delegate { directory = directoryBox.Text; pathSummary.Text = directory; pathSummary.ToolTip = directory; if (pathError != null) pathError.Visibility = Visibility.Collapsed; };
            editor.Children.Add(directoryBox);
            editor.Children.Add(Text("可直接输入完整目录；安装程序会检查此位置。", 10, Muted, new Thickness(0, 6, 0, 0)));
            optionStack.Children.Add(editor);
            change.Content = editor.Visibility == Visibility.Visible ? "收起" : "更改";
            change.Click += delegate {
                advancedExpanded = editor.Visibility != Visibility.Visible;
                editor.Visibility = advancedExpanded ? Visibility.Visible : Visibility.Collapsed;
                change.Content = advancedExpanded ? "收起" : "更改";
                if (advancedExpanded) { directoryBox.Focus(); directoryBox.SelectAll(); }
            };
            options.Child = optionStack;
            page.Children.Add(options);
            pathError = Text("请输入安装位置。", 12, BrushOf(153, 74, 58), new Thickness(1, 7, 0, 0));
            pathError.Visibility = Visibility.Collapsed;
            page.Children.Add(pathError);
            shortcutBox = Check("创建桌面和开始菜单快捷方式", createShortcuts);
            shortcutBox.Margin = new Thickness(1, 12, 0, 0);
            shortcutBox.Checked += delegate { createShortcuts = true; };
            shortcutBox.Unchecked += delegate { createShortcuts = false; };
            page.Children.Add(shortcutBox);
            if (!String.IsNullOrWhiteSpace(environment.InstalledVersion)) page.Children.Add(Text("已检测到版本 " + Clean(environment.InstalledVersion, 80) + "，将更新此应用。", 11, Muted, new Thickness(1, 9, 0, 0)));
            if (!environment.WebView2Present) page.Children.Add(Notice("需要 Microsoft WebView2。安装该组件时需联网获取微软运行时。", false));
            if (environment.Preview) page.Children.Add(Notice("当前为界面预览，不会写入应用、快捷方式或安装记录。", false));
            pageHost.Content = Scroll(page);
            StackPanel footer = new StackPanel { Margin = new Thickness(0, 14, 0, 0) };
            primaryButton = MakeButton(environment.Preview ? "预览安装流程  →" : "开始安装  →", true);
            primaryButton.IsDefault = true;
            primaryButton.Click += delegate { RequestInstall(); };
            footer.Children.Add(primaryButton);
            footer.Children.Add(Text(environment.Preview ? "仅预览安装器的界面与状态" : "安装前会检查目录和运行环境", 10, Muted, new Thickness(0, 9, 0, 0), TextAlignment.Center));
            footerHost.Content = footer;
            if (editPath) Dispatcher.BeginInvoke(new Action(delegate { directoryBox.Focus(); directoryBox.SelectAll(); }));
        }

        private void RequestInstall()
        {
            if (installing || succeeded) return;
            if (directoryBox != null) directory = directoryBox.Text.Trim();
            if (String.IsNullOrWhiteSpace(directory)) {
                RenderWelcome(true);
                pathError.Text = "请输入完整安装目录，再开始安装。";
                pathError.Visibility = Visibility.Visible;
                return;
            }
            InstallOptions options = new InstallOptions { InstallDirectory = directory, CreateShortcuts = createShortcuts, LaunchAfterInstall = true };
            Action<InstallOptions> handler = InstallRequested;
            if (handler == null) { ShowError("安装服务尚未连接，请关闭后重新运行安装器。"); return; }
            installError = null;
            RenderProgress();
            SetInstalling(true);
            try { handler(options); }
            catch (Exception ex) { ShowError(ex.Message); }
        }

        private void RenderProgress()
        {
            StackPanel page = Page(environment.Preview ? "正在预览安装流程" : "小柚正在整理新家", "稍等片刻，我们会在这里显示安装进展。");
            Border card = new Border { Background = Brushes.White, CornerRadius = new CornerRadius(16), BorderThickness = new Thickness(1), BorderBrush = Line, Padding = new Thickness(22), Margin = new Thickness(0, 21, 0, 0) };
            StackPanel body = new StackPanel();
            stepMarkers = new Border[3]; stepNumbers = new TextBlock[3]; stepLabels = new TextBlock[3];
            for (int i = 0; i < 3; i++) {
                Grid row = new Grid { Margin = new Thickness(0, 0, 0, i == 2 ? 0 : 20) };
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(38) });
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                stepNumbers[i] = Text((i + 1).ToString(), 12, Muted, new Thickness(0), TextAlignment.Center);
                stepNumbers[i].VerticalAlignment = VerticalAlignment.Center;
                stepMarkers[i] = new Border { Width = 26, Height = 26, Background = Pale, CornerRadius = new CornerRadius(13), Child = stepNumbers[i], HorizontalAlignment = HorizontalAlignment.Left };
                stepLabels[i] = Text(StageName(i), 13, Muted);
                stepLabels[i].VerticalAlignment = VerticalAlignment.Center;
                Grid.SetColumn(stepLabels[i], 1);
                row.Children.Add(stepMarkers[i]); row.Children.Add(stepLabels[i]);
                body.Children.Add(row);
            }
            ProgressBar bar = new ProgressBar { IsIndeterminate = true, Height = 5, Foreground = Green, Background = Pale, BorderThickness = new Thickness(0), Margin = new Thickness(0, 27, 0, 18) };
            AutomationProperties.SetName(bar, "安装正在进行，进度由安装服务报告");
            body.Children.Add(bar);
            progressTitle = Text(environment.Preview ? "预览 · 准备安装" : "准备安装", 15, Ink);
            progressTitle.FontWeight = FontWeights.SemiBold;
            progressDetail = Text("正在等待安装服务报告当前步骤。", 12, Muted, new Thickness(0, 9, 0, 0));
            body.Children.Add(progressTitle); body.Children.Add(progressDetail);
            card.Child = body;
            page.Children.Add(card);
            if (!environment.WebView2Present) page.Children.Add(Notice("此设备需要 WebView2；获取微软运行时的步骤需要联网。", false));
            pageHost.Content = Scroll(page);
            progressFootnote = Text("安装进行中，请保持此窗口打开。", 11, Muted, new Thickness(0, 18, 0, 4));
            footerHost.Content = progressFootnote;
            currentStep = 0;
            UpdateSteps();
        }

        private void UpdateSteps()
        {
            if (stepMarkers == null) return;
            for (int i = 0; i < 3; i++) {
                stepMarkers[i].Background = i <= currentStep ? Green : Pale;
                stepNumbers[i].Foreground = i <= currentStep ? Brushes.White : Muted;
                stepNumbers[i].Text = i < currentStep ? "✓" : (i + 1).ToString();
                stepLabels[i].Foreground = i <= currentStep ? Ink : Muted;
                stepLabels[i].FontWeight = i == currentStep ? FontWeights.SemiBold : FontWeights.Normal;
            }
        }

        private void RenderError()
        {
            progressTitle = null;
            StackPanel page = Page(environment.Preview ? "预览遇到了一点问题" : "安装暂未完成", "可以重试，或返回修改安装位置。");
            page.Children.Add(Notice(installError, true));
            page.Children.Add(Text("安装位置", 11, Muted, new Thickness(0, 23, 0, 5)));
            page.Children.Add(Text(directory, 12, Ink));
            page.Children.Add(Text("本次错误不会被当作安装成功。", 11, Muted, new Thickness(0, 16, 0, 0)));
            pageHost.Content = Scroll(page);
            StackPanel footer = new StackPanel { Margin = new Thickness(0, 18, 0, 0) };
            primaryButton = MakeButton(environment.Preview ? "重试预览" : "重试安装", true);
            primaryButton.IsDefault = true;
            primaryButton.Click += delegate { RequestInstall(); };
            footer.Children.Add(primaryButton);
            Button back = MakeButton("返回并修改路径", false);
            back.Margin = new Thickness(0, 9, 0, 0);
            back.Click += delegate { advancedExpanded = true; RenderWelcome(true); };
            footer.Children.Add(back);
            footerHost.Content = footer;
        }

        private void RenderSuccess()
        {
            progressTitle = null;
            bool preview = environment.Preview || installedResult.Preview;
            StackPanel page = Page(preview ? "界面流程预览完成" : "小柚已经准备好了", preview ? "你已经看完了安装器的主要流程。" : "接下来，打开应用，开始你的专注时光。");
            Border seal = new Border { Width = 58, Height = 58, CornerRadius = new CornerRadius(29), Background = Pale, HorizontalAlignment = HorizontalAlignment.Left, Margin = new Thickness(0, 22, 0, 17), Child = Text("✓", 28, Green, new Thickness(0, 6, 0, 0), TextAlignment.Center) };
            page.Children.Add(seal);
            page.Children.Add(Text(preview ? "本次没有安装或启动应用。" : BuildInfo.ProductName + " v" + BuildInfo.ProductVersion + " 已安装", 17, Ink));
            page.Children.Add(Text(preview ? "这是界面预览，不代表实际安装结果。" : "安装位置", 12, Muted, new Thickness(0, 14, 0, 6)));
            if (!preview) page.Children.Add(Text(Clean(installedResult.InstallDirectory, 2000), 12, Ink));
            launchError = new Border { Background = BrushOf(253, 241, 232), CornerRadius = new CornerRadius(12), Padding = new Thickness(14), Margin = new Thickness(0, 18, 0, 0), Visibility = Visibility.Collapsed };
            page.Children.Add(launchError);
            launchBox = Check("完成后打开 " + BuildInfo.ProductName, installedResult.LaunchAfterInstall);
            launchBox.Visibility = preview ? Visibility.Collapsed : Visibility.Visible;
            launchBox.Margin = new Thickness(0, 24, 0, 0);
            launchBox.Checked += delegate { UpdateCompletionButton(); };
            launchBox.Unchecked += delegate { UpdateCompletionButton(); };
            page.Children.Add(launchBox);
            pageHost.Content = Scroll(page);
            StackPanel footer = new StackPanel { Margin = new Thickness(0, 18, 0, 0) };
            primaryButton = MakeButton("", true);
            primaryButton.IsDefault = true;
            primaryButton.Click += delegate {
                if (launching) return;
                if (preview || launchBox.IsChecked != true) { Close(); return; }
                Action handler = LaunchRequested;
                if (handler == null) { ShowLaunchError("启动服务尚未连接，可从安装目录手动打开应用。"); return; }
                launching = true;
                primaryButton.IsEnabled = false;
                primaryButton.Content = "正在打开…";
                launchBox.IsEnabled = false;
                launchError.Visibility = Visibility.Collapsed;
                try { handler(); }
                catch (Exception ex) { ShowLaunchError(ex.Message); }
            };
            footer.Children.Add(primaryButton);
            if (!preview) {
                Button finish = MakeButton("仅完成，暂不打开", false);
                finish.Margin = new Thickness(0, 9, 0, 0);
                finish.Click += delegate { Close(); };
                footer.Children.Add(finish);
            }
            footerHost.Content = footer;
            UpdateCompletionButton();
        }

        private void UpdateCompletionButton()
        {
            if (primaryButton == null || launching) return;
            bool preview = environment.Preview || (installedResult != null && installedResult.Preview);
            primaryButton.Content = preview ? "完成预览" : (launchBox != null && launchBox.IsChecked == true ? "完成并打开  →" : "完成");
        }

        private void SetInstalling(bool value)
        {
            installing = value;
            closeButton.IsEnabled = !value || environment.Preview;
            closeButton.ToolTip = value && !environment.Preview ? "安装进行中，请等待当前步骤完成" : "关闭安装器";
            chromeStatus.Text = environment.Preview ? "界面预览 · 不执行安装" : (value ? "安装进行中" : "为当前 Windows 用户安装");
        }

        private void OnUi(Action action)
        {
            if (Dispatcher.CheckAccess()) action();
            else if (!Dispatcher.HasShutdownStarted) Dispatcher.BeginInvoke(action);
        }

        private void FocusPrimary()
        {
            Dispatcher.BeginInvoke(new Action(delegate { if (primaryButton != null && primaryButton.IsEnabled) primaryButton.Focus(); }));
        }

        private void DragChrome(object sender, MouseButtonEventArgs args)
        {
            if (args.ChangedButton != MouseButton.Left || args.ButtonState != MouseButtonState.Pressed) return;
            DependencyObject target = args.OriginalSource as DependencyObject;
            while (target != null && target != sender) {
                if (target is ButtonBase || target is TextBoxBase) return;
                target = target is Visual ? VisualTreeHelper.GetParent(target) : LogicalTreeHelper.GetParent(target);
            }
            try { DragMove(); } catch (InvalidOperationException) { }
        }

        private static StackPanel Page(string title, string description)
        {
            StackPanel page = new StackPanel { Margin = new Thickness(0, 17, 7, 0) };
            TextBlock heading = Text(title, 27, Ink);
            heading.FontWeight = FontWeights.SemiBold;
            page.Children.Add(heading);
            page.Children.Add(Text(description, 12, Muted, new Thickness(0, 11, 0, 18)));
            return page;
        }

        private static ScrollViewer Scroll(UIElement content)
        {
            return new ScrollViewer { Content = content, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled, VerticalScrollBarVisibility = ScrollBarVisibility.Auto, PanningMode = PanningMode.VerticalOnly, Focusable = false };
        }

        private static UIElement Feature(string geometry, string title, string detail)
        {
            Grid row = new Grid { Margin = new Thickness(0, 7, 0, 7) };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(45) });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            Border tile = new Border { Width = 33, Height = 33, Background = Pale, CornerRadius = new CornerRadius(10), HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Top };
            DrawingPath icon = new DrawingPath { Data = Geometry.Parse(geometry), Stroke = Green, StrokeThickness = 1.45, StrokeStartLineCap = PenLineCap.Round, StrokeEndLineCap = PenLineCap.Round, StrokeLineJoin = PenLineJoin.Round, Width = 21, Height = 21, Stretch = Stretch.Uniform };
            tile.Child = icon;
            row.Children.Add(tile);
            StackPanel words = new StackPanel();
            TextBlock name = Text(title, 13, Ink); name.FontWeight = FontWeights.SemiBold;
            words.Children.Add(name);
            words.Children.Add(Text(detail, 11, Muted, new Thickness(0, 5, 0, 0)));
            Grid.SetColumn(words, 1);
            row.Children.Add(words);
            return row;
        }

        private static Border Notice(string message, bool error)
        {
            return new Border { Background = error ? BrushOf(253, 241, 232) : Pale, CornerRadius = new CornerRadius(12), Padding = new Thickness(13, 11, 13, 11), Margin = new Thickness(0, 14, 0, 0), Child = Text(message, 12, error ? BrushOf(143, 81, 63) : Green) };
        }

        private static CheckBox Check(string label, bool selected)
        {
            CheckBox box = new CheckBox { Content = label, IsChecked = selected, Foreground = Ink, FontSize = 12, VerticalContentAlignment = VerticalAlignment.Center, Padding = new Thickness(3, 1, 0, 1), Cursor = Cursors.Hand };
            AutomationProperties.SetName(box, label);
            return box;
        }

        private static Button ChromeButton(string label, string accessibleName)
        {
            Button button = MakeButton(label, false);
            button.Width = 31; button.Height = 29; button.MinHeight = 0;
            button.Padding = new Thickness(0); button.FontSize = label == "×" ? 22 : 15;
            button.Margin = new Thickness(4, 0, 0, 0);
            button.Background = Brushes.Transparent; button.BorderBrush = Brushes.Transparent;
            button.ToolTip = accessibleName;
            AutomationProperties.SetName(button, accessibleName);
            return button;
        }

        private static Button MakeButton(string label, bool primary)
        {
            Button button = new Button { Content = label, Background = primary ? Green : Brushes.Transparent, Foreground = primary ? Brushes.White : Green, BorderBrush = primary ? Green : Line, BorderThickness = new Thickness(1), Padding = new Thickness(17, 11, 17, 11), MinHeight = 42, FontSize = 14, FontWeight = FontWeights.SemiBold, Cursor = Cursors.Hand, HorizontalContentAlignment = HorizontalAlignment.Center, VerticalContentAlignment = VerticalAlignment.Center };
            ControlTemplate template = new ControlTemplate(typeof(Button));
            FrameworkElementFactory frame = new FrameworkElementFactory(typeof(Border));
            frame.Name = "Frame";
            frame.SetValue(Border.CornerRadiusProperty, new CornerRadius(11));
            frame.SetValue(Border.BackgroundProperty, new TemplateBindingExtension(Control.BackgroundProperty));
            frame.SetValue(Border.BorderBrushProperty, new TemplateBindingExtension(Control.BorderBrushProperty));
            frame.SetValue(Border.BorderThicknessProperty, new TemplateBindingExtension(Control.BorderThicknessProperty));
            FrameworkElementFactory content = new FrameworkElementFactory(typeof(ContentPresenter));
            content.SetValue(ContentPresenter.HorizontalAlignmentProperty, HorizontalAlignment.Center);
            content.SetValue(ContentPresenter.VerticalAlignmentProperty, VerticalAlignment.Center);
            content.SetValue(ContentPresenter.MarginProperty, new TemplateBindingExtension(Control.PaddingProperty));
            content.SetValue(ContentPresenter.RecognizesAccessKeyProperty, true);
            frame.AppendChild(content);
            template.VisualTree = frame;
            Trigger hover = new Trigger { Property = UIElement.IsMouseOverProperty, Value = true };
            hover.Setters.Add(new Setter(Border.BackgroundProperty, primary ? GreenHover : Pale, "Frame"));
            template.Triggers.Add(hover);
            Trigger focus = new Trigger { Property = UIElement.IsKeyboardFocusedProperty, Value = true };
            focus.Setters.Add(new Setter(Border.BorderBrushProperty, BrushOf(38, 69, 38), "Frame"));
            focus.Setters.Add(new Setter(Border.BorderThicknessProperty, new Thickness(2), "Frame"));
            template.Triggers.Add(focus);
            Trigger disabled = new Trigger { Property = UIElement.IsEnabledProperty, Value = false };
            disabled.Setters.Add(new Setter(UIElement.OpacityProperty, 0.5, "Frame"));
            template.Triggers.Add(disabled);
            button.Template = template;
            return button;
        }

        private static string StageName(int step)
        {
            return step == 0 ? "校验安装环境" : step == 1 ? "安装 Token 喵" : "验证安装结果";
        }

        private static SolidColorBrush BrushOf(byte red, byte green, byte blue)
        {
            SolidColorBrush brush = new SolidColorBrush(Color.FromRgb(red, green, blue));
            brush.Freeze();
            return brush;
        }

        private static TextBlock Text(string value, double size, Brush color)
        {
            return Text(value, size, color, new Thickness(0));
        }

        private static TextBlock Text(string value, double size, Brush color, Thickness margin)
        {
            return Text(value, size, color, margin, TextAlignment.Left);
        }

        private static TextBlock Text(string value, double size, Brush color, Thickness margin, TextAlignment align)
        {
            return new TextBlock { Text = value ?? String.Empty, FontSize = size, Foreground = color, Margin = margin, TextWrapping = TextWrapping.Wrap, TextAlignment = align, LineHeight = size * 1.55 };
        }

        private static string Clean(string value, int limit)
        {
            if (String.IsNullOrEmpty(value)) return String.Empty;
            StringBuilder text = new StringBuilder();
            foreach (char c in value) {
                if ((!Char.IsControl(c) || c == '\n' || c == '\t') && !(c >= '\u202a' && c <= '\u202e') && !(c >= '\u2066' && c <= '\u2069')) text.Append(c);
                if (text.Length >= limit) break;
            }
            return text.ToString().Trim();
        }
    }
}
