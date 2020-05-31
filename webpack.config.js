const path = require('path');
const glob = require('glob-all')
const { CleanWebpackPlugin } = require('clean-webpack-plugin');
const MiniCssExtractPlugin = require('mini-css-extract-plugin');
const TerserPlugin = require('terser-webpack-plugin');
const PurgeCssPlugin = require('purgecss-webpack-plugin')
const FontminPlugin = require('fontmin-webpack')

const PATHS = {
    src: path.join(__dirname, 'templates'),
    web: path.join(__dirname, 'web'),
}

module.exports = {
    entry: {
        bundle: './web/js/index.js',
    },
    output: {
        path: path.resolve(__dirname, 'static'),
        filename: 'bundle.min.js'
    },
    plugins: [
        new CleanWebpackPlugin(),
        new MiniCssExtractPlugin({ filename: '[name].min.css' }),
        new PurgeCssPlugin({
            paths:
                glob.sync([`${PATHS.src}/**/*`, `${PATHS.web}/**/*`], { nodir: true })
        }),
        new FontminPlugin({
            autodetect: true,
        }),
    ],
    module: {
        rules: [{
            test: /\.scss$/,
            use: [MiniCssExtractPlugin.loader, 'css-loader', 'sass-loader'],
        },
        {
            test: /\.(woff(2)?|ttf|eot|svg)(\?v=\d+\.\d+\.\d+)?$/,
            use: [
                {
                    loader: 'file-loader',
                    options: {
                        name: '[name].[ext]',
                        outputPath: 'fonts/'
                    }
                }
            ]
        },
        {
            test: /\.m?js$/,
            exclude: /node_modules/,
            use: {
                loader: 'babel-loader',
                options: {
                    presets: ['@babel/preset-env']
                }
            }
        }
        ]
    },
    optimization: {
        usedExports: true,
        minimizer: [new TerserPlugin()],
    },
}